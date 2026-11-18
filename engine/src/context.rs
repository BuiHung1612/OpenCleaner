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
        Context {
            home,
            vars,
            data_dir,
        }
    }
}
