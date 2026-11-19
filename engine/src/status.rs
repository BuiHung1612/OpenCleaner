//! Dashboard readings, taken in-process (no collector to start). CPU figures, for the
//! machine and per process, are the change since the previous reading; the very first
//! reading samples twice, 250 ms apart.

use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    pub hardware: Hardware,
    /// e.g. "3d 4h"
    pub uptime: String,
    /// The same in seconds, for the app to word in the user's language
    pub uptime_secs: Option<u64>,
    pub cpu: Cpu,
    pub memory: Memory,
    pub disks: Vec<Disk>,
    /// Bytes in the Trash / Recycle Bin, when readable.
    pub trash_size: Option<u64>,
    pub top_processes: Vec<Process>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Hardware {
    pub model: String,
    pub cpu_model: String,
    pub total_ram: String,
    pub os_version: String,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Cpu {
    /// percent of all cores
    pub usage: f64,
    pub cores: usize,
    pub load1: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Memory {
    pub total: u64,
    pub used: u64,
    pub used_percent: f64,
    /// Not in use at all (file caches count as used here, unlike in `used`)
    pub free: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Disk {
    pub mount: String,
    pub total: u64,
    pub used: u64,
    pub used_percent: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Process {
    pub pid: i32,
    pub name: String,
    /// percent of one core, like Activity Monitor
    pub cpu: f64,
    pub memory_bytes: u64,
}

/// What a reading compares against.
struct Sample {
    at: Instant,
    cpu: platform::CpuTicks,
    procs: Vec<(i32, u64)>, // pid, cpu time in ns
}

static LAST: Mutex<Option<Sample>> = Mutex::new(None);

pub fn read() -> Status {
    let mut last = LAST.lock().unwrap();
    let first = last.is_none();
    if first {
        *last = Some(sample());
        std::thread::sleep(Duration::from_millis(250));
    }
    let prev = last.take().unwrap();
    let now = sample();
    let elapsed_ns = now.at.duration_since(prev.at).as_nanos().max(1) as f64;

    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let cpu = Cpu {
        usage: platform::cpu_usage(&prev.cpu, &now.cpu),
        cores,
        load1: platform::load1(),
    };

    let mut procs: Vec<Process> = platform::processes()
        .into_iter()
        .map(|(pid, name, cpu_ns, memory_bytes)| {
            let before = prev.procs.iter().find(|p| p.0 == pid).map(|p| p.1);
            let cpu = before
                .map(|b| cpu_ns.saturating_sub(b) as f64 / elapsed_ns * 100.0)
                .unwrap_or(0.0);
            Process {
                pid,
                name,
                cpu,
                memory_bytes,
            }
        })
        .collect();
    // The first reading spans only those 250 ms, which in the app is its own launch: it would
    // show OpenCleaner busy (~25 %) until the next reading. Later readings span the time
    // since the previous one and list the app like any other process.
    if first {
        procs.retain(|p| p.pid != std::process::id() as i32);
    }
    procs.sort_by(|a, b| {
        b.cpu
            .total_cmp(&a.cpu)
            .then(b.memory_bytes.cmp(&a.memory_bytes))
    });
    procs.truncate(10);

    *last = Some(now);
    drop(last);

    let memory = platform::memory();
    let disks = platform::disks();
    Status {
        hardware: platform::hardware(),
        uptime: platform::uptime().map(format_uptime).unwrap_or_default(),
        uptime_secs: platform::uptime().map(|d| d.as_secs()),
        trash_size: platform::trash_size(),
        top_processes: procs,
        cpu,
        memory,
        disks,
    }
}

fn sample() -> Sample {
    Sample {
        at: Instant::now(),
        cpu: platform::cpu_ticks(),
        procs: platform::processes()
            .into_iter()
            .map(|(pid, _, ns, _)| (pid, ns))
            .collect(),
    }
}

fn format_uptime(d: Duration) -> String {
    let mins = d.as_secs() / 60;
    let (days, hours, minutes) = (mins / 1440, mins / 60 % 24, mins % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

/// Only digits and dots ("2.1.328"): a version, not a name (some tools install each
/// version as an executable named after it).
fn version_like(name: &str) -> bool {
    name.contains(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// A process name people recognise: for a version-like name, the nearest folder of its
/// executable that is a name ("claude" for ~/.local/share/claude/versions/2.1.328).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn process_name(name: String, exe: impl FnOnce() -> Option<std::path::PathBuf>) -> String {
    if !version_like(&name) {
        return name;
    }
    let Some(exe) = exe() else { return name };
    exe.iter()
        .rev()
        .filter_map(|c| c.to_str())
        .find(|c| !version_like(c) && !matches!(*c, "versions" | "bin" | "MacOS" | "Current"))
        .map_or(name, str::to_owned)
}

fn format_gb(bytes: u64) -> String {
    format!("{} GB", (bytes as f64 / (1u64 << 30) as f64).round())
}

// libc marks its Mach bindings deprecated in favour of the mach2 crate; they are the same
// calls, and one fewer dependency.
#[cfg(target_os = "macos")]
#[allow(deprecated)]
mod platform {
    use super::{Disk, Hardware, Memory, format_gb};
    use std::ffi::{CStr, CString, c_int};
    use std::time::Duration;

    pub type CpuTicks = [u64; 4]; // user, system, idle, nice

    unsafe extern "C" {
        fn getloadavg(loadavg: *mut f64, nelem: c_int) -> c_int;
    }

    /// The name people know the Mac by ("Mac mini (2023)"); Apple silicon Macs keep it in
    /// the device tree. Intel Macs do not: the caller falls back to hw.model.
    fn product_name() -> Option<String> {
        use std::ffi::{c_char, c_void};
        #[link(name = "IOKit", kind = "framework")]
        unsafe extern "C" {
            fn IORegistryEntryFromPath(main_port: u32, path: *const c_char) -> u32;
            fn IORegistryEntryCreateCFProperty(
                entry: u32,
                key: *const c_void,
                allocator: *const c_void,
                options: u32,
            ) -> *const c_void;
            fn IOObjectRelease(object: u32) -> i32;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            fn CFStringCreateWithCString(
                alloc: *const c_void,
                s: *const c_char,
                encoding: u32,
            ) -> *const c_void;
            fn CFGetTypeID(cf: *const c_void) -> usize;
            fn CFDataGetTypeID() -> usize;
            fn CFDataGetLength(data: *const c_void) -> isize;
            fn CFDataGetBytePtr(data: *const c_void) -> *const u8;
            fn CFRelease(cf: *const c_void);
        }
        unsafe {
            let entry = IORegistryEntryFromPath(0, c"IODeviceTree:/product".as_ptr());
            if entry == 0 {
                return None;
            }
            let key =
                CFStringCreateWithCString(std::ptr::null(), c"product-name".as_ptr(), 0x0800_0100);
            let data = IORegistryEntryCreateCFProperty(entry, key, std::ptr::null(), 0);
            CFRelease(key);
            IOObjectRelease(entry);
            if data.is_null() {
                return None;
            }
            let name = (CFGetTypeID(data) == CFDataGetTypeID()).then(|| {
                let bytes = std::slice::from_raw_parts(
                    CFDataGetBytePtr(data),
                    CFDataGetLength(data).max(0) as usize,
                );
                String::from_utf8_lossy(bytes)
                    .trim_end_matches('\0')
                    .to_string()
            });
            CFRelease(data);
            name.filter(|n| !n.is_empty())
        }
    }

    fn sysctl_string(name: &str) -> String {
        let c = CString::new(name).unwrap();
        let mut len = 0usize;
        unsafe {
            if libc::sysctlbyname(
                c.as_ptr(),
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            ) != 0
                || len == 0
            {
                return String::new();
            }
            let mut buf = vec![0u8; len];
            if libc::sysctlbyname(
                c.as_ptr(),
                buf.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            ) != 0
            {
                return String::new();
            }
            CStr::from_bytes_until_nul(&buf)
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        }
    }

    fn sysctl_value<T: Copy + Default>(name: &str) -> Option<T> {
        let c = CString::new(name).unwrap();
        let mut v = T::default();
        let mut len = std::mem::size_of::<T>();
        let rc = unsafe {
            libc::sysctlbyname(
                c.as_ptr(),
                (&mut v as *mut T).cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        (rc == 0).then_some(v)
    }

    pub fn cpu_ticks() -> CpuTicks {
        let mut info: libc::host_cpu_load_info = unsafe { std::mem::zeroed() };
        let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
        let rc = unsafe {
            libc::host_statistics(
                libc::mach_host_self(),
                libc::HOST_CPU_LOAD_INFO,
                (&mut info as *mut libc::host_cpu_load_info).cast(),
                &mut count,
            )
        };
        if rc != 0 {
            return [0; 4];
        }
        let t = info.cpu_ticks;
        [
            t[libc::CPU_STATE_USER as usize] as u64,
            t[libc::CPU_STATE_SYSTEM as usize] as u64,
            t[libc::CPU_STATE_IDLE as usize] as u64,
            t[libc::CPU_STATE_NICE as usize] as u64,
        ]
    }

    pub fn cpu_usage(a: &CpuTicks, b: &CpuTicks) -> f64 {
        let d: Vec<u64> = (0..4).map(|i| b[i].wrapping_sub(a[i])).collect();
        let total = d.iter().sum::<u64>();
        if total == 0 {
            return 0.0;
        }
        (d[0] + d[1] + d[3]) as f64 / total as f64 * 100.0
    }

    pub fn load1() -> Option<f64> {
        let mut l = [0f64; 3];
        (unsafe { getloadavg(l.as_mut_ptr(), 3) } > 0).then_some(l[0])
    }

    pub fn memory() -> Memory {
        let total: u64 = sysctl_value("hw.memsize").unwrap_or(0);
        let mut vm: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let rc = unsafe {
            libc::host_statistics64(
                libc::mach_host_self(),
                libc::HOST_VM_INFO64,
                (&mut vm as *mut libc::vm_statistics64).cast(),
                &mut count,
            )
        };
        let page: u64 = sysctl_value::<u64>("hw.pagesize").unwrap_or(16384);
        // Activity Monitor's "Memory Used": app memory + wired + compressed
        let used = if rc == 0 {
            let app = (vm.internal_page_count as u64).saturating_sub(vm.purgeable_count as u64);
            (app + vm.wire_count as u64 + vm.compressor_page_count as u64) * page
        } else {
            0
        };
        let free = if rc == 0 {
            vm.free_count as u64 * page
        } else {
            0
        };
        Memory {
            total,
            used,
            used_percent: if total > 0 {
                used as f64 / total as f64 * 100.0
            } else {
                0.0
            },
            free,
        }
    }

    pub fn disks() -> Vec<Disk> {
        // the Data volume holds what users fill; it shares its container with "/"
        let path = if std::path::Path::new("/System/Volumes/Data").exists() {
            "/System/Volumes/Data"
        } else {
            "/"
        };
        let c = CString::new(path).unwrap();
        let mut st: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
            return Vec::new();
        }
        let total = st.f_blocks * st.f_bsize as u64;
        let used = total.saturating_sub(st.f_bavail * st.f_bsize as u64);
        vec![Disk {
            mount: "/".into(),
            total,
            used,
            used_percent: if total > 0 {
                used as f64 / total as f64 * 100.0
            } else {
                0.0
            },
        }]
    }

    pub fn hardware() -> Hardware {
        let total: u64 = sysctl_value("hw.memsize").unwrap_or(0);
        Hardware {
            model: product_name().unwrap_or_else(|| sysctl_string("hw.model")),
            cpu_model: sysctl_string("machdep.cpu.brand_string"),
            total_ram: format_gb(total),
            os_version: format!("macOS {}", sysctl_string("kern.osproductversion")),
        }
    }

    pub fn uptime() -> Option<Duration> {
        let boot: libc::timeval = sysctl_value("kern.boottime")?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        now.checked_sub(Duration::from_secs(boot.tv_sec as u64))
    }

    pub fn trash_size() -> Option<u64> {
        let home = std::env::var_os("HOME")?;
        let r = crate::size::measure(
            &std::path::Path::new(&home).join(".Trash"),
            &Default::default(),
        )
        .ok()?;
        (!r.permission_only || r.bytes > 0).then_some(r.bytes)
    }

    /// Only asked for version-like names (see `process_name`).
    fn pid_path(pid: i32) -> Option<std::path::PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        buf.truncate(n.max(0) as usize);
        (n > 0).then(|| std::ffi::OsString::from_vec(buf).into())
    }

    /// (pid, name, cpu time in ns, resident bytes) of every process we may inspect.
    pub fn processes() -> Vec<(i32, String, u64, u64)> {
        let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
        unsafe { libc::mach_timebase_info(&mut tb) };
        let (numer, denom) = (tb.numer.max(1) as u128, tb.denom.max(1) as u128);

        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0i32; n as usize + 64];
        let n =
            unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as c_int) };
        pids.truncate(n.max(0) as usize);

        let mut out = Vec::with_capacity(pids.len());
        for pid in pids {
            if pid <= 0 {
                continue;
            }
            let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_taskinfo>() as c_int;
            let got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTASKINFO,
                    0,
                    (&mut info as *mut libc::proc_taskinfo).cast(),
                    size,
                )
            };
            if got != size {
                continue;
            }
            let mut name = [0u8; 256];
            let len = unsafe { libc::proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
            let name = String::from_utf8_lossy(&name[..len.max(0) as usize]).into_owned();
            let name = super::process_name(name, || pid_path(pid));
            // task times are Mach absolute time units (not ns on Apple Silicon)
            let ticks = info.pti_total_user as u128 + info.pti_total_system as u128;
            out.push((
                pid,
                name,
                (ticks * numer / denom) as u64,
                info.pti_resident_size,
            ));
        }
        out
    }
}

#[cfg(windows)]
mod platform {
    use super::{Disk, Hardware, Memory, format_gb};
    use std::time::Duration;
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows_sys::Win32::System::SystemInformation::{
        GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    pub type CpuTicks = [u64; 3]; // idle, kernel (includes idle), user

    fn ft(f: FILETIME) -> u64 {
        (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
    }

    pub fn cpu_ticks() -> CpuTicks {
        let (mut idle, mut kernel, mut user) =
            unsafe { std::mem::zeroed::<(FILETIME, FILETIME, FILETIME)>() };
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return [0; 3];
        }
        [ft(idle), ft(kernel), ft(user)]
    }

    pub fn cpu_usage(a: &CpuTicks, b: &CpuTicks) -> f64 {
        let idle = b[0].wrapping_sub(a[0]);
        let total = b[1].wrapping_sub(a[1]) + b[2].wrapping_sub(a[2]);
        if total == 0 {
            0.0
        } else {
            (total - idle.min(total)) as f64 / total as f64 * 100.0
        }
    }

    pub fn load1() -> Option<f64> {
        None
    }

    pub fn memory() -> Memory {
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } == 0 {
            return Memory::default();
        }
        let used = m.ullTotalPhys - m.ullAvailPhys;
        Memory {
            total: m.ullTotalPhys,
            used,
            used_percent: used as f64 / m.ullTotalPhys.max(1) as f64 * 100.0,
            free: m.ullAvailPhys,
        }
    }

    pub fn disks() -> Vec<Disk> {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\";
        let wide: Vec<u16> = drive.encode_utf16().chain([0]).collect();
        let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut total_free) }
            == 0
        {
            return Vec::new();
        }
        let used = total - total_free;
        vec![Disk {
            mount: drive,
            total,
            used,
            used_percent: used as f64 / total.max(1) as f64 * 100.0,
        }]
    }

    pub fn hardware() -> Hardware {
        Hardware {
            model: std::env::var("COMPUTERNAME").unwrap_or_default(),
            cpu_model: std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default(),
            total_ram: format_gb(memory().total),
            os_version: "Windows".into(),
        }
    }

    pub fn uptime() -> Option<Duration> {
        Some(Duration::from_millis(unsafe { GetTickCount64() }))
    }

    /// Bytes in the Recycle Bins of all drives.
    pub fn trash_size() -> Option<u64> {
        use windows_sys::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};
        let mut info: SHQUERYRBINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHQUERYRBINFO>() as u32;
        (unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) } == 0)
            .then_some(info.i64Size.max(0) as u64)
    }

    /// (pid, name, cpu time in ns, working set bytes) of the processes we may inspect.
    pub fn processes() -> Vec<(i32, String, u64, u64)> {
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        };
        use windows_sys::Win32::System::ProcessStatus::{
            K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
        };
        use windows_sys::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        let mut out = Vec::new();
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = unsafe { Process32FirstW(snap, &mut entry) };
        while ok != 0 {
            let pid = entry.th32ProcessID;
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
            if !h.is_null() {
                let (mut created, mut exited, mut kernel, mut user) =
                    unsafe { std::mem::zeroed::<(FILETIME, FILETIME, FILETIME, FILETIME)>() };
                let mut mem: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
                mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
                let times = unsafe {
                    GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user)
                } != 0;
                let memory = unsafe { K32GetProcessMemoryInfo(h, &mut mem, mem.cb) } != 0;
                if times {
                    // FILETIME counts 100 ns
                    let cpu_ns = (ft(kernel) + ft(user)) * 100;
                    out.push((
                        pid as i32,
                        name,
                        cpu_ns,
                        if memory { mem.WorkingSetSize as u64 } else { 0 },
                    ));
                }
                unsafe { CloseHandle(h) };
            }
            ok = unsafe { Process32NextW(snap, &mut entry) };
        }
        unsafe { CloseHandle(snap) };
        out
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::{Disk, Hardware, Memory};
    use std::time::Duration;
    pub type CpuTicks = [u64; 1];
    pub fn cpu_ticks() -> CpuTicks {
        [0]
    }
    pub fn cpu_usage(_: &CpuTicks, _: &CpuTicks) -> f64 {
        0.0
    }
    pub fn load1() -> Option<f64> {
        None
    }
    pub fn memory() -> Memory {
        Memory::default()
    }
    pub fn disks() -> Vec<Disk> {
        Vec::new()
    }
    pub fn hardware() -> Hardware {
        Hardware::default()
    }
    pub fn uptime() -> Option<Duration> {
        None
    }
    pub fn trash_size() -> Option<u64> {
        None
    }
    pub fn processes() -> Vec<(i32, String, u64, u64)> {
        Vec::new()
    }
}

#[cfg(test)]
mod name_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn version_named_processes_take_their_folder_name() {
        let claude = || {
            Some(PathBuf::from(
                "/Users/a/.local/share/claude/versions/2.1.328",
            ))
        };
        assert_eq!(process_name("2.1.328".into(), claude), "claude");
        assert_eq!(process_name("Safari".into(), || unreachable!()), "Safari");
        assert_eq!(process_name("1.0".into(), || None), "1.0");
        assert!(!version_like("..."));
        assert!(!version_like("node22"));
    }
}
