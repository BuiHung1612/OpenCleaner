//! Startup Items: what starts when the user logs in, turning off the user's own ones, and
//! (macOS) which apps hold privacy permissions.
//!
//! macOS: launchd property lists in ~/Library/LaunchAgents (the user's: may be turned off)
//! and /Library/LaunchAgents, /Library/LaunchDaemons (the system's: shown only, changing
//! them needs administrator rights). Login items added in System Settings (SMAppService)
//! live in a database only root can read, without a public API to list them: not listed.
//! Windows: the Run keys of the user and the machine, and the two Startup folders.
//!
//! Turning an item off records it in the history so it can be put back: the plist or
//! shortcut goes to the Trash / Recycle Bin, a Run value's command is kept in the ledger.

use crate::context::Context;
use crate::guard::{self, Purpose, Refusal};
use crate::ledger::{self, Line};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub name: String,
    /// The plist or shortcut; for a Run value, its registry key.
    pub path: PathBuf,
    /// launchd label, or the Run value's name.
    pub label: String,
    pub program: String,
    /// "user": OpenCleaner may turn it off; "system": needs administrator rights.
    pub scope: &'static str,
    pub enabled: bool,
}

/// A friendly name: the app the program lives in, else the program's file name, else the
/// label.
fn display_name(program: &str, label: &str) -> String {
    let p = Path::new(program);
    p.ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "app"))
        .or(Some(p).filter(|p| !p.as_os_str().is_empty()))
        .and_then(|a| a.file_stem())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| label.to_string())
}

/// A file that may be turned off: a plain file sitting directly in `dir`, inside the
/// home, with no link on the way (the System Junk guards, with `dir` as the root).
pub fn check_user_file(ctx: &Context, path: &Path, dir: &Path) -> Result<(), Refusal> {
    if path.parent() != Some(dir) {
        return Err("not directly in the startup folder");
    }
    guard::check_for(path, dir, ctx, Purpose::Clean)?;
    if !std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
        return Err("not a plain file");
    }
    Ok(())
}

fn record(ctx: &Context, item: &Item) {
    let mut ledger = ledger::Writer::open(ctx);
    let now = ledger::now();
    ledger.write(&Line::Startup {
        session: now,
        path: item.path.clone(),
        label: item.label.clone(),
        program: item.program.clone(),
    });
    ledger.write(&Line::Session {
        id: now,
        command: "startup".into(),
        started: now,
        finished: now,
        items: 1,
        bytes: 0,
        failed: 0,
    });
}

#[cfg(target_os = "macos")]
pub use mac::{agents_dir, disable, list, permissions, read_plist, read_tcc};

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use serde_json::{Value, json};
    use std::process::Command;

    pub fn agents_dir(ctx: &Context) -> PathBuf {
        ctx.home.join("Library/LaunchAgents")
    }

    pub fn read_plist(path: &Path, scope: &'static str) -> Option<Item> {
        let v = plist::Value::from_file(path).ok()?;
        let d = v.as_dictionary()?;
        let text = |k: &str| d.get(k).and_then(|v| v.as_string()).map(str::to_owned);
        // a plist without a label (a leftover) still shows under its file name
        let label = text("Label").filter(|l| !l.is_empty()).unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
        let args: Vec<String> = d
            .get("ProgramArguments")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_string().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let exe = text("Program")
            .or_else(|| args.first().cloned())
            .unwrap_or_default();
        let program = if args.is_empty() {
            exe.clone()
        } else {
            args.join(" ")
        };
        Some(Item {
            name: display_name(&exe, &label),
            path: path.to_path_buf(),
            label,
            program,
            scope,
            enabled: !d
                .get("Disabled")
                .and_then(|v| v.as_boolean())
                .unwrap_or(false),
        })
    }

    fn list_dir(dir: &Path, scope: &'static str) -> Vec<Item> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut items: Vec<Item> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "plist"))
            .filter_map(|p| read_plist(&p, scope))
            .collect();
        items.sort_by_key(|i| i.name.to_lowercase());
        items
    }

    pub fn list(ctx: &Context) -> Vec<Item> {
        let mut out = list_dir(&agents_dir(ctx), "user");
        for dir in ["/Library/LaunchAgents", "/Library/LaunchDaemons"] {
            out.extend(list_dir(Path::new(dir), "system"));
        }
        out
    }

    /// Stops the agent now (`launchctl bootout`) and moves its plist to the Trash, so it
    /// does not start at the next login. `_label` is for Windows Run values.
    pub fn disable(ctx: &Context, path: &Path, _label: &str) -> Result<(), String> {
        check_user_file(ctx, path, &agents_dir(ctx))?;
        let item = read_plist(path, "user").ok_or("not a launchd property list")?;
        let uid = unsafe { libc::getuid() };
        // an agent that is not loaded fails here; the file going is what matters
        let _ = Command::new("/bin/launchctl")
            .arg("bootout")
            .arg(format!("gui/{uid}"))
            .arg(path)
            .output();
        crate::trash::move_to_trash(path).map_err(|e| e.to_string())?;
        record(ctx, &item);
        Ok(())
    }

    /// TCC services shown, by the short name the app uses.
    const SERVICES: &[(&str, &str)] = &[
        ("kTCCServiceCamera", "camera"),
        ("kTCCServiceMicrophone", "microphone"),
        ("kTCCServiceScreenCapture", "screen"),
        ("kTCCServiceSystemPolicyAllFiles", "files"),
        ("kTCCServiceAccessibility", "accessibility"),
    ];

    /// The allowed grants in one TCC database. A missing database has none; one that
    /// cannot be read (no Full Disk Access) is an error.
    pub fn read_tcc(db: &Path) -> Result<Vec<Value>, String> {
        match std::fs::metadata(db) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.to_string()),
            Ok(_) => {}
        }
        let names: Vec<String> = SERVICES.iter().map(|(s, _)| format!("'{s}'")).collect();
        let query = format!(
            "SELECT service, client FROM access WHERE auth_value = 2 AND service IN ({})",
            names.join(",")
        );
        let out = Command::new("/usr/bin/sqlite3")
            .args(["-readonly", "-json"])
            .arg(db)
            .arg(query)
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        let rows: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap_or_default();
        Ok(rows
            .iter()
            .filter_map(|r| {
                let service = SERVICES.iter().find(|(s, _)| r["service"] == *s)?.1;
                Some(json!({"service": service, "client": r["client"]}))
            })
            .collect())
    }

    /// Grants from the user's and the system's databases (camera and microphone live in
    /// the user's, the others in the system's). Reading them needs Full Disk Access.
    pub fn permissions(ctx: &Context) -> Value {
        let mut grants = Vec::new();
        for db in [
            ctx.home
                .join("Library/Application Support/com.apple.TCC/TCC.db"),
            PathBuf::from("/Library/Application Support/com.apple.TCC/TCC.db"),
        ] {
            match read_tcc(&db) {
                Ok(g) => grants.extend(g),
                Err(_) => return json!({"grants": [], "needs_full_disk_access": true}),
            }
        }
        json!({"grants": grants, "needs_full_disk_access": false})
    }
}

#[cfg(windows)]
pub use win::{disable, list};

#[cfg(windows)]
mod win {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
        RRF_NOEXPAND, RRF_RT_REG_BINARY, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey,
        RegDeleteKeyValueW, RegEnumValueW, RegGetValueW, RegOpenKeyExW,
    };

    const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const APPROVED: &str =
        "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved";
    const HKCU_RUN: &str = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn startup_dir(ctx: &Context) -> Option<PathBuf> {
        Some(
            ctx.vars
                .get("APPDATA")?
                .join("Microsoft\\Windows\\Start Menu\\Programs\\Startup"),
        )
    }

    /// A Run value's command as stored (environment variables not expanded).
    fn value(key: HKEY, sub: &str, name: &str) -> Option<String> {
        let (sub, name) = (wide(sub), wide(name));
        let mut buf = vec![0u16; 4096];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                key,
                sub.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Task Manager's on/off switch: an odd first byte under StartupApproved means off.
    fn approved(root: HKEY, kind: &str, name: &str) -> bool {
        let (sub, name) = (wide(&format!("{APPROVED}\\{kind}")), wide(name));
        let mut data = [0u8; 12];
        let mut len = data.len() as u32;
        let rc = unsafe {
            RegGetValueW(
                root,
                sub.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_BINARY,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut len,
            )
        };
        rc != ERROR_SUCCESS || data[0] & 1 == 0
    }

    fn run_values(root: HKEY, view: u32, scope: &'static str, shown: &str) -> Vec<Item> {
        let mut out = Vec::new();
        let mut key: HKEY = std::ptr::null_mut();
        let path = wide(RUN);
        if unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ | view, &mut key) }
            != ERROR_SUCCESS
        {
            return out;
        }
        let mut index = 0u32;
        loop {
            let mut name = [0u16; 16384];
            let mut len = name.len() as u32;
            let rc = unsafe {
                RegEnumValueW(
                    key,
                    index,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if rc != ERROR_SUCCESS {
                break;
            }
            index += 1;
            let label = String::from_utf16_lossy(&name[..len as usize]);
            let Some(program) = value(key, "", &label) else {
                continue;
            };
            // the program is the quoted part, or the first word
            let exe = match program.strip_prefix('"') {
                Some(rest) => rest.split('"').next(),
                None => program.split(' ').next(),
            }
            .unwrap_or("")
            .to_string();
            out.push(Item {
                name: display_name(&exe, &label),
                path: PathBuf::from(shown),
                enabled: approved(root, "Run", &label),
                label,
                program,
                scope,
            });
        }
        unsafe { RegCloseKey(key) };
        out
    }

    fn folder(dir: &Path, scope: &'static str, root: HKEY) -> Vec<Item> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|e| !e.file_name().eq_ignore_ascii_case("desktop.ini"))
            .map(|e| {
                let label = e.file_name().to_string_lossy().into_owned();
                Item {
                    name: e
                        .path()
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    path: e.path(),
                    program: e.path().to_string_lossy().into(),
                    enabled: approved(root, "StartupFolder", &label),
                    label,
                    scope,
                }
            })
            .collect()
    }

    pub fn list(ctx: &Context) -> Vec<Item> {
        let mut out = run_values(HKEY_CURRENT_USER, 0, "user", HKCU_RUN);
        if let Some(dir) = startup_dir(ctx) {
            out.extend(folder(&dir, "user", HKEY_CURRENT_USER));
        }
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            let shown = "HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Run";
            for item in run_values(HKEY_LOCAL_MACHINE, view, "system", shown) {
                if !out
                    .iter()
                    .any(|i| i.path == item.path && i.label == item.label)
                {
                    out.push(item);
                }
            }
        }
        if let Some(data) = ctx.vars.get("PROGRAMDATA") {
            let dir = data.join("Microsoft\\Windows\\Start Menu\\Programs\\StartUp");
            out.extend(folder(&dir, "system", HKEY_LOCAL_MACHINE));
        }
        out
    }

    /// Removes the user's Run value `label` (its command is kept in the ledger), or moves
    /// a shortcut in the user's Startup folder to the Recycle Bin.
    pub fn disable(ctx: &Context, path: &Path, label: &str) -> Result<(), String> {
        if path == Path::new(HKCU_RUN) {
            let program = value(HKEY_CURRENT_USER, RUN, label).ok_or("no such startup item")?;
            let (sub, name) = (wide(RUN), wide(label));
            let rc = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, sub.as_ptr(), name.as_ptr()) };
            if rc != ERROR_SUCCESS {
                return Err(format!("the registry refused (error {rc})"));
            }
            record(
                ctx,
                &Item {
                    name: label.into(),
                    path: path.into(),
                    label: label.into(),
                    program,
                    scope: "user",
                    enabled: true,
                },
            );
            return Ok(());
        }
        let dir = startup_dir(ctx).ok_or("no Startup folder")?;
        check_user_file(ctx, path, &dir)?;
        crate::trash::move_to_trash(path).map_err(|e| e.to_string())?;
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        record(
            ctx,
            &Item {
                name: name.clone().into(),
                path: path.into(),
                label: name.into(),
                program: String::new(),
                scope: "user",
                enabled: true,
            },
        );
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn list(_ctx: &Context) -> Vec<Item> {
    Vec::new()
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn disable(_ctx: &Context, _path: &Path, _label: &str) -> Result<(), String> {
    Err("not supported here".into())
}
