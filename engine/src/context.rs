//! Where the engine works: the user's home, path variables for rules, and where it
//! keeps its own data. Tests build one around a temporary directory.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct Context {
    pub home: PathBuf,
    /// `%NAME%` values for Windows rules (LOCALAPPDATA, TEMP…)
    pub vars: HashMap<String, PathBuf>,
    /// History and other engine files.
    pub data_dir: PathBuf,
}

impl Context {
    pub fn from_env() -> Context {
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let home = var("HOME")
            .or_else(|| var("USERPROFILE"))
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut vars = HashMap::new();
        for k in [
            "LOCALAPPDATA",
            "APPDATA",
            "TEMP",
            "USERPROFILE",
            "ProgramData",
        ] {
            if let Some(v) = var(k) {
                vars.insert(k.to_ascii_uppercase(), v);
            }
        }
        let data_dir = if cfg!(target_os = "macos") {
            home.join("Library/Application Support/OpenCleaner")
        } else if let Some(local) = vars.get("LOCALAPPDATA") {
            local.join("OpenCleaner")
        } else {
            home.join(".local/share/opencleaner")
        };
        // Windows: where Explorer really keeps Desktop, Documents… (often moved into OneDrive)
        #[cfg(windows)]
        for (key, value) in USER_FOLDERS {
            if let Some(p) = shell_folder(value) {
                vars.insert((*key).into(), p);
            }
        }
        Context {
            home,
            vars,
            data_dir,
        }
    }

    /// One of the user's own folders ("Desktop", "Documents", "Downloads"…): where Windows
    /// keeps it (OneDrive may hold Desktop and Documents), else `home/name`.
    pub fn user_folder(&self, name: &str) -> PathBuf {
        self.vars.get(&name.to_ascii_uppercase()).cloned().unwrap_or_else(|| self.home.join(name))
    }
}

/// (var name, value under Explorer's "Shell Folders")
#[cfg(windows)]
const USER_FOLDERS: &[(&str, &str)] = &[
    ("DESKTOP", "Desktop"),
    ("DOCUMENTS", "Personal"),
    ("PICTURES", "My Pictures"),
    ("MUSIC", "My Music"),
    ("VIDEOS", "My Video"),
    ("DOWNLOADS", "{374DE290-123F-4565-9164-39C4925E467B}"),
];

#[cfg(windows)]
fn shell_folder(value: &str) -> Option<PathBuf> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, name) = (wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders"), wide(value));
    let mut buf = [0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr().cast(), &mut bytes)
    };
    let end = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..end]));
    (rc == 0 && end > 0 && path.is_dir()).then_some(path)
}
