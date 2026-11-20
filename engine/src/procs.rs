//! Which apps are running, so their live caches are left alone.
//!
//! macOS: the bundle id of each app whose own executable runs (in its Contents/MacOS; a
//! helper app inside another counts as itself). Tools that only live inside an app, like
//! sourcekit-lsp or DeviceHub inside Xcode.app, do not make it count as open. Windows:
//! lower-cased executable names.

use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct Running {
    /// lower-cased id or executable name → the name to show (the app's name)
    names: HashMap<String, String>,
}

impl Running {
    pub fn snapshot() -> Running {
        Running {
            names: platform::running(),
        }
    }

    /// For tests: pretend these are running.
    pub fn of(names: &[&str]) -> Running {
        Running {
            names: names
                .iter()
                .map(|n| (n.to_ascii_lowercase(), n.to_string()))
                .collect(),
        }
    }

    /// Is any of `ids` running? Compared case-insensitively.
    /// The name of the first of `ids` that runs (case-insensitive), as people know it.
    pub fn any(&self, ids: &[String]) -> Option<String> {
        ids.iter()
            .find_map(|id| self.names.get(&id.to_ascii_lowercase()))
            .cloned()
    }

    /// The first of `ids` that runs, as given, and the name people know it by.
    pub fn which(&self, ids: &[String]) -> Option<(String, String)> {
        ids.iter()
            .find_map(|id| Some((id.clone(), self.names.get(&id.to_ascii_lowercase())?.clone())))
    }

    pub fn contains(&self, id: &str) -> bool {
        self.names.contains_key(&id.to_ascii_lowercase())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    pub fn running() -> HashMap<String, String> {
        let mut out = HashMap::new();
        let mut bundles: HashMap<PathBuf, Option<String>> = HashMap::new();
        for pid in pids() {
            let Some(exe) = pid_path(pid) else { continue };
            // the app whose Contents/MacOS holds the executable: Chrome's helpers run inside
            // "Google Chrome Helper.app" and quit with Chrome; editors run sourcekit-lsp from
            // inside Xcode.app while Xcode itself is closed
            for bundle in exe
                .ancestors()
                .filter(|p| p.extension().is_some_and(|e| e == "app"))
                .filter(|b| exe.parent() == Some(b.join("Contents/MacOS").as_path()))
            {
                let id = bundles
                    .entry(bundle.to_path_buf())
                    .or_insert_with(|| bundle_id(bundle));
                if let Some(id) = id {
                    let app = bundle.file_stem().unwrap_or_default().to_string_lossy();
                    out.insert(id.to_ascii_lowercase(), app.into_owned());
                }
            }
            if let Some(name) = exe.file_name() {
                let name = name.to_string_lossy();
                out.entry(name.to_ascii_lowercase())
                    .or_insert_with(|| name.into_owned());
            }
        }
        out
    }

    fn pids() -> Vec<i32> {
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return Vec::new();
        }
        let mut pids = vec![0i32; n as usize + 64];
        let n =
            unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
        pids.truncate(n.max(0) as usize);
        pids.retain(|&p| p > 0);
        pids
    }

    fn pid_path(pid: i32) -> Option<PathBuf> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(buf)))
    }

    fn bundle_id(bundle: &Path) -> Option<String> {
        let info = plist::Value::from_file(bundle.join("Contents/Info.plist")).ok()?;
        info.as_dictionary()?
            .get("CFBundleIdentifier")?
            .as_string()
            .map(str::to_owned)
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    pub fn running() -> HashMap<String, String> {
        let mut out = HashMap::new();
        let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = unsafe { Process32FirstW(snap, &mut entry) };
        while ok != 0 {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            let shown = name.strip_suffix(".exe").unwrap_or(&name).to_string();
            out.insert(name.to_ascii_lowercase(), shown);
            ok = unsafe { Process32NextW(snap, &mut entry) };
        }
        unsafe { CloseHandle(snap) };
        out
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub fn running() -> std::collections::HashMap<String, String> {
        Default::default()
    }
}
