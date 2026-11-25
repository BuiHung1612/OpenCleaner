//! Process Doctor: which of the user's processes are stuck, heavy or left behind, and which
//! are worth quitting. Only the user's own processes are ever listed or quit; the system's,
//! the app's own and its parents are never offered.
//!
//! A reading takes two snapshots 400 ms apart (the change in CPU time gives "busy now"),
//! then `analyze` (pure, tested without real processes) turns them into findings.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GIB: u64 = 1 << 30;
/// Resident memory from which a process counts as heavy: 1 GiB or this share of the RAM,
/// whichever is larger; half a GiB when the machine is short of memory.
const HEAVY_MIN: u64 = GIB;
const HEAVY_SHARE: f64 = 0.08;
/// Stuck in a loop: busy now, and busy for most of its life, for at least 10 minutes.
const SPIN_NOW: f64 = 90.0;
const SPIN_LIFETIME: f64 = 80.0;
const SPIN_MIN_AGE: u64 = 10 * 60;
const IDLE_CPU: f64 = 1.0;
const LONG_RUNNING: u64 = 24 * 3600;
/// An idle dev tool is only worth a mention when it holds some memory.
const IDLE_TOOL_MIN: u64 = 100 << 20;
const LEFT_BEHIND: u64 = 3600;
/// Swap in use from which the machine counts as short of memory.
const PRESSURE_SWAP: f64 = 0.6;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    Sleeping,
    Stopped,
    Zombie,
}

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    /// Program and arguments (Windows: the executable name only)
    pub command: String,
    /// The app a GUI process belongs to (macOS: the enclosing .app)
    pub app: Option<String>,
    /// Owned by the user, so ours to quit
    pub mine: bool,
    pub state: State,
    /// Unix seconds
    pub started: u64,
    pub rss: u64,
    /// CPU time used since it started
    pub cpu_ns: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Machine {
    pub total_ram: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    /// Swap is mostly full: the machine is slow for lack of memory
    pub under_pressure: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Quitting it is safe and worth it: stuck in a loop, or left behind
    Quit,
    /// Heavy, old or idle: the user decides
    Review,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Dead, but its parent never collected it (cannot be quit; the parent can)
    Zombie,
    /// Suspended
    Stopped,
    /// Busy in a loop for a long time
    Spinning,
    HeavyMemory,
    /// A dev tool whose parent is gone
    LeftBehind,
    /// A dev tool running for over a day doing nothing
    IdleDevTool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Finding {
    pub pid: i32,
    /// Start time, which `quit` checks so a reused pid is never hit
    pub started: u64,
    pub name: String,
    /// The app or tool it belongs to, as people know it
    pub app: String,
    pub command: String,
    pub memory_bytes: u64,
    /// percent of one core right now
    pub cpu: f64,
    pub age_secs: u64,
    pub verdict: Verdict,
    pub reasons: Vec<Reason>,
    pub can_quit: bool,
}

/// Several processes of one app, or a lot of memory in one app: Chrome's tabs, dozens of
/// `claude`, language servers.
#[derive(Clone, Debug, Serialize)]
pub struct Group {
    pub app: String,
    pub count: usize,
    pub memory_bytes: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub machine: Machine,
    pub findings: Vec<Finding>,
    pub groups: Vec<Group>,
    /// What quitting every finding would free, roughly
    pub reclaimable: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Exited,
    /// Asked to quit but still running: offer to force it
    StillRunning,
}

/// Parts of a command that mark a development tool; `true` = it runs as a daemon by design
/// (orphaned from the start), so only its idleness counts.
const DEV_TOOLS: &[(&str, bool)] = &[
    ("gradledaemon", true),
    ("kotlincompiledaemon", true),
    ("fork-server", true),
    ("next dev", false),
    ("next-server", false),
    ("react-native start", false),
    ("metro", false),
    ("jest-worker", false),
    ("jest ", false),
    ("vitest", false),
    ("vite", false),
    ("webpack", false),
    ("nodemon", false),
    ("tsserver", false),
    ("esbuild", false),
    (" logcat", false),
    ("tail -f", false),
    ("playwright", false),
    ("qemu-system", false),
];

/// Never offered, whoever owns them.
const PROTECTED: &[&str] = &[
    "launchd",
    "kernel_task",
    "windowserver",
    "loginwindow",
    "finder",
    "dock",
    "systemuiserver",
    "controlcenter",
    "notificationcenter",
    "opencleaner",
    "oce",
    "system",
    "registry",
    "smss",
    "csrss",
    "wininit",
    "winlogon",
    "services",
    "lsass",
    "svchost",
    "dwm",
    "explorer",
];

fn is_protected(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let n = n.strip_suffix(".exe").unwrap_or(&n);
    PROTECTED.contains(&n)
}

/// `Some(true)` for a daemon-by-design dev tool, `Some(false)` for another dev tool.
fn dev_tool(p: &Proc) -> Option<bool> {
    let hay = format!("{} {}", p.name, p.command).to_ascii_lowercase();
    DEV_TOOLS
        .iter()
        .find(|(part, _)| hay.contains(part))
        .map(|&(_, daemon)| daemon)
}

pub fn analyze(
    procs: &[Proc],
    cpu: &HashMap<i32, f64>,
    now: u64,
    machine: &Machine,
    me: i32,
) -> Report {
    let by_pid: HashMap<i32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    // this process and every parent above it (the shell and terminal running it)
    let mut safe: HashSet<i32> = HashSet::from([me]);
    let mut at = me;
    while let Some(p) = by_pid.get(&at) {
        if p.ppid <= 1 || !safe.insert(p.ppid) {
            break;
        }
        at = p.ppid;
    }

    let heavy_min = if machine.under_pressure {
        HEAVY_MIN / 2
    } else {
        HEAVY_MIN.max((machine.total_ram as f64 * HEAVY_SHARE) as u64)
    };

    let mut findings = Vec::new();
    let mut groups: HashMap<String, (usize, u64)> = HashMap::new();
    for p in procs {
        if !p.mine || p.pid <= 1 || safe.contains(&p.pid) || is_protected(&p.name) {
            continue;
        }
        let app = p.app.clone().unwrap_or_else(|| p.name.clone());
        let g = groups.entry(app.clone()).or_default();
        g.0 += 1;
        g.1 += p.rss;

        let age = now.saturating_sub(p.started);
        let busy = cpu.get(&p.pid).copied().unwrap_or(0.0);
        let lifetime = if age > 0 {
            p.cpu_ns as f64 / 1e9 / age as f64 * 100.0
        } else {
            0.0
        };
        let orphan = p.ppid <= 1 || !by_pid.contains_key(&p.ppid);
        let dev = dev_tool(p);

        let mut reasons = Vec::new();
        let mut verdict = Verdict::Review;
        match p.state {
            State::Zombie => reasons.push(Reason::Zombie),
            State::Stopped => reasons.push(Reason::Stopped),
            _ => {}
        }
        if busy >= SPIN_NOW && lifetime >= SPIN_LIFETIME && age >= SPIN_MIN_AGE {
            reasons.push(Reason::Spinning);
            verdict = Verdict::Quit;
        }
        if p.rss >= heavy_min {
            reasons.push(Reason::HeavyMemory);
        }
        if let Some(daemon) = dev.filter(|_| busy < IDLE_CPU) {
            if orphan && !daemon && age >= LEFT_BEHIND {
                reasons.push(Reason::LeftBehind);
                verdict = Verdict::Quit;
            } else if age >= LONG_RUNNING && p.rss >= IDLE_TOOL_MIN {
                reasons.push(Reason::IdleDevTool);
            }
        }
        if reasons.is_empty() {
            continue;
        }
        findings.push(Finding {
            pid: p.pid,
            started: p.started,
            name: p.name.clone(),
            app,
            command: p.command.clone(),
            memory_bytes: p.rss,
            cpu: busy,
            age_secs: age,
            verdict,
            can_quit: p.state != State::Zombie,
            reasons,
        });
    }
    findings.sort_by(|a, b| {
        a.verdict
            .cmp(&b.verdict)
            .then(b.memory_bytes.cmp(&a.memory_bytes))
    });
    findings.truncate(40);
    let reclaimable = findings.iter().map(|f| f.memory_bytes).sum();

    let mut groups: Vec<Group> = groups
        .into_iter()
        .filter(|(_, (count, bytes))| *count >= 4 || *bytes >= GIB)
        .map(|(app, (count, memory_bytes))| Group {
            app,
            count,
            memory_bytes,
        })
        .collect();
    groups.sort_by_key(|g| std::cmp::Reverse(g.memory_bytes));
    groups.truncate(10);

    Report {
        machine: machine.clone(),
        findings,
        groups,
        reclaimable,
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A reading of this machine, now.
pub fn read() -> Report {
    let first = platform::snapshot(false);
    let t0 = std::time::Instant::now();
    std::thread::sleep(Duration::from_millis(400));
    let second = platform::snapshot(true);
    let elapsed = t0.elapsed().as_nanos().max(1) as f64;
    let before: HashMap<i32, u64> = first.iter().map(|p| (p.pid, p.cpu_ns)).collect();
    let cpu = second
        .iter()
        .filter_map(|p| {
            let b = before.get(&p.pid)?;
            Some((p.pid, p.cpu_ns.saturating_sub(*b) as f64 / elapsed * 100.0))
        })
        .collect();
    analyze(
        &second,
        &cpu,
        now_secs(),
        &platform::machine(),
        std::process::id() as i32,
    )
}

/// Asks the process to quit (`force`: kills it at once), after checking it is the user's own
/// process that was listed: same start time (a reused pid is refused), not a protected or
/// parent process. Then waits up to 3 s for it to go.
pub fn quit(pid: i32, started: u64, force: bool) -> Result<Outcome, String> {
    let procs = platform::snapshot(false);
    let p = procs
        .iter()
        .find(|p| p.pid == pid)
        .ok_or("It has already quit.")?;
    if p.started != started {
        return Err("That is a different process now.".into());
    }
    if !p.mine || pid <= 1 || is_protected(&p.name) || is_ancestor(&procs, pid) {
        return Err("OpenCleaner does not quit that process.".into());
    }
    platform::signal(pid, force)?;
    for _ in 0..12 {
        std::thread::sleep(Duration::from_millis(250));
        let gone = platform::snapshot(false)
            .iter()
            .all(|q| q.pid != pid || q.started != started || q.state == State::Zombie);
        if gone {
            return Ok(Outcome::Exited);
        }
    }
    Ok(Outcome::StillRunning)
}

/// When `pid` started (Unix seconds), if it runs: what `quit` is given with the pid.
pub fn start_of(pid: i32) -> Option<u64> {
    platform::snapshot(false)
        .into_iter()
        .find(|p| p.pid == pid)
        .map(|p| p.started)
}

/// Is `pid` this process or one of its parents?
fn is_ancestor(procs: &[Proc], pid: i32) -> bool {
    let by_pid: HashMap<i32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut at = std::process::id() as i32;
    for _ in 0..64 {
        if at == pid {
            return true;
        }
        match by_pid.get(&at) {
            Some(p) if p.ppid > 1 => at = p.ppid,
            _ => return false,
        }
    }
    false
}

// libc marks its Mach bindings deprecated in favour of the mach2 crate; same calls, one
// fewer dependency (as in status.rs).
#[cfg(target_os = "macos")]
#[allow(deprecated)]
mod platform {
    use super::{Machine, Proc, State};
    use std::os::raw::c_int;

    pub fn snapshot(full: bool) -> Vec<Proc> {
        let mut tb = libc::mach_timebase_info { numer: 0, denom: 0 };
        unsafe { libc::mach_timebase_info(&mut tb) };
        let (numer, denom) = (tb.numer.max(1) as u128, tb.denom.max(1) as u128);
        let uid = unsafe { libc::getuid() };

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
            let mut bsd: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let size = std::mem::size_of::<libc::proc_bsdinfo>() as c_int;
            let got = unsafe {
                libc::proc_pidinfo(
                    pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&mut bsd as *mut libc::proc_bsdinfo).cast(),
                    size,
                )
            };
            if got != size {
                continue;
            }
            let zombie = bsd.pbi_status == 5;
            // a zombie has no task: no memory, no CPU
            let mut task: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
            let tsize = std::mem::size_of::<libc::proc_taskinfo>() as c_int;
            let have_task = !zombie
                && unsafe {
                    libc::proc_pidinfo(
                        pid,
                        libc::PROC_PIDTASKINFO,
                        0,
                        (&mut task as *mut libc::proc_taskinfo).cast(),
                        tsize,
                    )
                } == tsize;
            if !zombie && !have_task {
                continue;
            }
            let mine = bsd.pbi_uid == uid;
            let name = cstr(&bsd.pbi_name);
            let name = if name.is_empty() {
                cstr(&bsd.pbi_comm)
            } else {
                name
            };
            let exe = if mine { pid_path(pid) } else { None };
            let app = exe.as_deref().and_then(app_of);
            let name = crate::status::process_name(name, || exe.clone());
            let command = if mine && full && !zombie {
                args(pid).unwrap_or_default()
            } else {
                String::new()
            };
            let ticks = task.pti_total_user as u128 + task.pti_total_system as u128;
            out.push(Proc {
                pid,
                ppid: bsd.pbi_ppid as i32,
                name,
                command,
                app,
                mine,
                state: match bsd.pbi_status {
                    5 => State::Zombie,
                    4 => State::Stopped,
                    2 => State::Running,
                    _ => State::Sleeping,
                },
                started: bsd.pbi_start_tvsec,
                rss: if have_task { task.pti_resident_size } else { 0 },
                cpu_ns: (ticks * numer / denom) as u64,
            });
        }
        out
    }

    fn cstr(bytes: &[libc::c_char]) -> String {
        let bytes: Vec<u8> = bytes
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn pid_path(pid: i32) -> Option<std::path::PathBuf> {
        use std::os::unix::ffi::OsStringExt;
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        buf.truncate(n.max(0) as usize);
        (n > 0).then(|| std::ffi::OsString::from_vec(buf).into())
    }

    /// The outermost .app that holds the executable ("Google Chrome Helper" inside Chrome
    /// counts as Chrome).
    fn app_of(exe: &std::path::Path) -> Option<String> {
        exe.ancestors()
            .filter(|p| p.extension().is_some_and(|e| e == "app"))
            .last()
            .and_then(|b| b.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
    }

    /// Program and arguments, from the kernel's argument block (argc, program path, then
    /// the arguments, NUL separated).
    fn args(pid: i32) -> Option<String> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size = 0usize;
        let mut ask = |buf: *mut libc::c_void, size: &mut usize| unsafe {
            libc::sysctl(mib.as_mut_ptr(), 3, buf, size, std::ptr::null_mut(), 0)
        };
        if ask(std::ptr::null_mut(), &mut size) != 0 || size < 4 {
            return None;
        }
        let mut buf = vec![0u8; size];
        if ask(buf.as_mut_ptr().cast(), &mut size) != 0 {
            return None;
        }
        buf.truncate(size);
        let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?).max(0) as usize;
        let mut parts = buf[4..].split(|&b| b == 0);
        let program = parts.next().map(String::from_utf8_lossy)?;
        let args: Vec<String> = parts
            .filter(|a| !a.is_empty())
            .take(argc)
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        let mut command = if args.is_empty() {
            program.into_owned()
        } else {
            args.join(" ")
        };
        command.truncate(
            command
                .char_indices()
                .nth(400)
                .map_or(command.len(), |(i, _)| i),
        );
        Some(command)
    }

    pub fn machine() -> Machine {
        fn sysctl<T: Default>(name: &str) -> Option<T> {
            let name = std::ffi::CString::new(name).ok()?;
            let mut value = T::default();
            let mut size = std::mem::size_of::<T>();
            let rc = unsafe {
                libc::sysctlbyname(
                    name.as_ptr(),
                    (&mut value as *mut T).cast(),
                    &mut size,
                    std::ptr::null_mut(),
                    0,
                )
            };
            (rc == 0).then_some(value)
        }
        #[derive(Default)]
        #[repr(C)]
        struct Swap {
            total: u64,
            avail: u64,
            used: u64,
            pagesize: u32,
            encrypted: bool,
        }
        let swap: Swap = sysctl("vm.swapusage").unwrap_or_default();
        Machine {
            total_ram: sysctl::<u64>("hw.memsize").unwrap_or(0),
            swap_used: swap.used,
            swap_total: swap.total,
            under_pressure: swap.total > 0
                && swap.used as f64 >= swap.total as f64 * super::PRESSURE_SWAP,
        }
    }

    pub fn signal(pid: i32, force: bool) -> Result<(), String> {
        let sig = if force { libc::SIGKILL } else { libc::SIGTERM };
        if unsafe { libc::kill(pid, sig) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::{Machine, Proc, State};
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };

    fn ft(f: FILETIME) -> u64 {
        (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64
    }

    /// "Mine" = we may terminate it, which for a normal user means their own processes.
    pub fn snapshot(_full: bool) -> Vec<Proc> {
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
            let exe = String::from_utf16_lossy(&entry.szExeFile[..len]);
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
                    0,
                    pid,
                )
            };
            if !handle.is_null() {
                let (mut created, mut exited, mut kernel, mut user): (
                    FILETIME,
                    FILETIME,
                    FILETIME,
                    FILETIME,
                ) = unsafe { std::mem::zeroed() };
                let timed = unsafe {
                    GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user)
                } != 0;
                let mut mem: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
                mem.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
                let sized = unsafe { K32GetProcessMemoryInfo(handle, &mut mem, mem.cb) } != 0;
                unsafe { CloseHandle(handle) };
                if timed {
                    // FILETIME counts 100 ns from 1601; Unix time from 1970
                    let started = (ft(created) / 10_000_000).saturating_sub(11_644_473_600);
                    out.push(Proc {
                        pid: pid as i32,
                        ppid: entry.th32ParentProcessID as i32,
                        name: exe.strip_suffix(".exe").unwrap_or(&exe).to_string(),
                        command: exe,
                        app: None,
                        mine: true,
                        state: State::Running,
                        started,
                        rss: if sized { mem.WorkingSetSize as u64 } else { 0 },
                        cpu_ns: (ft(kernel) + ft(user)) * 100,
                    });
                }
            }
            ok = unsafe { Process32NextW(snap, &mut entry) };
        }
        unsafe { CloseHandle(snap) };
        out
    }

    pub fn machine() -> Machine {
        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let _ = unsafe { GlobalMemoryStatusEx(&mut m) };
        // the page file beyond RAM is what Windows swaps to
        let swap_total = m.ullTotalPageFile.saturating_sub(m.ullTotalPhys);
        let swap_used = m
            .ullTotalPageFile
            .saturating_sub(m.ullAvailPageFile)
            .saturating_sub(m.ullTotalPhys.saturating_sub(m.ullAvailPhys));
        Machine {
            total_ram: m.ullTotalPhys,
            swap_used,
            swap_total,
            under_pressure: swap_total > 0
                && swap_used as f64 >= swap_total as f64 * super::PRESSURE_SWAP,
        }
    }

    pub fn signal(pid: i32, force: bool) -> Result<(), String> {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string()]);
        if force {
            cmd.arg("/F");
        }
        let out = cmd
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output()
            .map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stdout).trim().to_string())
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    use super::{Machine, Proc};

    pub fn snapshot(_full: bool) -> Vec<Proc> {
        Vec::new()
    }
    pub fn machine() -> Machine {
        Machine::default()
    }
    pub fn signal(_pid: i32, _force: bool) -> Result<(), String> {
        Err("Not supported on this system.".into())
    }
}
