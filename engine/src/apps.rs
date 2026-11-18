//! Uninstaller: the installed apps, and what each leaves in the user's Library.

use crate::context::Context;
use crate::guard::Purpose;
use crate::picks::{Action, Pick};
use crate::procs::Running;
use crate::rules;
use crate::size;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Debug, Default, Serialize)]
pub struct App {
    pub name: String,
    /// macOS: CFBundleIdentifier. Windows: the app's key under Uninstall in the registry.
    pub bundle_id: String,
    pub version: String,
    /// The bundle (macOS) or install folder (Windows, may be empty).
    pub path: PathBuf,
    pub running: bool,
    /// Windows: who made it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    /// Windows: the size the installer registered, for apps without a known folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_size: Option<u64>,
    /// Windows: the vendor's uninstaller command line (the proper way to remove an app).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uninstall: Option<String>,
}

/// Apps that come with the system, or that are OpenCleaner itself.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const KEEP: &[&str] = &["com.apple.Safari", "io.github.opencleaner"];

/// Installed apps, by name. Sizes come separately (`sizes`), since reading them takes a
/// walk of every bundle.
#[cfg(windows)]
pub fn list(_ctx: &Context, _running: &Running) -> Vec<App> {
    win::list()
}

#[cfg(not(windows))]
pub fn list(ctx: &Context, running: &Running) -> Vec<App> {
    let mut apps = Vec::new();
    for dir in app_dirs(ctx) {
        let Ok(items) = std::fs::read_dir(&dir) else {
            continue;
        };
        for item in items.flatten() {
            let path = item.path();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() || !meta.is_dir() {
                continue;
            }
            if path.extension().is_some_and(|e| e == "app") {
                if let Some(app) = read_app(&path, running) {
                    apps.push(app);
                }
            } else if dir == Path::new("/Applications") {
                // one folder level, like /Applications/Utilities or a vendor folder
                let Ok(inner) = std::fs::read_dir(&path) else {
                    continue;
                };
                for sub in inner.flatten().map(|e| e.path()) {
                    if sub.extension().is_some_and(|e| e == "app")
                        && std::fs::symlink_metadata(&sub).is_ok_and(|m| m.is_dir())
                        && let Some(app) = read_app(&sub, running)
                    {
                        apps.push(app);
                    }
                }
            }
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.path == b.path);
    apps
}

#[cfg(not(windows))]
fn app_dirs(ctx: &Context) -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![
            PathBuf::from("/Applications"),
            ctx.home.join("Applications"),
        ]
    } else {
        Vec::new()
    }
}

#[cfg(target_os = "macos")]
fn read_app(path: &Path, running: &Running) -> Option<App> {
    let info = plist::Value::from_file(path.join("Contents/Info.plist")).ok()?;
    let dict = info.as_dictionary()?;
    let text = |k: &str| dict.get(k).and_then(|v| v.as_string()).map(str::to_owned);
    let bundle_id = text("CFBundleIdentifier")?;
    if KEEP.iter().any(|k| k.eq_ignore_ascii_case(&bundle_id)) {
        return None;
    }
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(App {
        name: text("CFBundleDisplayName")
            .or_else(|| text("CFBundleName"))
            .filter(|n| !n.is_empty())
            .unwrap_or(stem),
        version: text("CFBundleShortVersionString").unwrap_or_default(),
        running: running.contains(&bundle_id),
        bundle_id,
        path: path.to_path_buf(),
        ..Default::default()
    })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read_app(_path: &Path, _running: &Running) -> Option<App> {
    None
}

/// Bytes each app bundle takes, in the order given.
///
/// Sizes are remembered between runs (`<data dir>/app-sizes.json`) against a stamp of the
/// bundle: an app that was not updated or replaced is not walked again. Known sizes are
/// reported at once through `progress`.
pub fn sizes(
    ctx: &Context,
    apps: &[PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&[u64]),
) -> Vec<u64> {
    let file = ctx.data_dir.join("app-sizes.json");
    let mut cache: HashMap<PathBuf, (String, u64)> = std::fs::read(&file)
        .ok()
        .and_then(|d| serde_json::from_slice(&d).ok())
        .unwrap_or_default();
    let stamps: Vec<Option<String>> = apps.iter().map(|p| stamp(p)).collect();
    let mut out: Vec<u64> = apps
        .iter()
        .zip(&stamps)
        .map(|(p, s)| match (cache.get(p), s) {
            (Some((was, bytes)), Some(now)) if was == now => *bytes,
            _ => 0,
        })
        .collect();
    let missing: Vec<usize> = (0..apps.len()).filter(|&i| out[i] == 0).collect();
    if missing.len() < apps.len() {
        progress(&out);
    }
    if !missing.is_empty() {
        let paths: Vec<PathBuf> = missing.iter().map(|&i| apps[i].clone()).collect();
        let base = out.clone();
        let measured = size::allocated_many(&paths, cancel, &mut |partial| {
            let mut now = base.clone();
            for (k, &i) in missing.iter().enumerate() {
                now[i] = partial[k];
            }
            progress(&now);
        });
        for (k, &i) in missing.iter().enumerate() {
            out[i] = measured[k];
        }
    }
    if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
        cache.clear();
        for ((p, s), b) in apps.iter().zip(stamps).zip(&out) {
            if let Some(s) = s {
                cache.insert(p.clone(), (s, *b));
            }
        }
        let _ = std::fs::create_dir_all(&ctx.data_dir);
        if let Ok(text) = serde_json::to_vec(&cache) {
            let _ = std::fs::write(&file, text);
        }
    }
    out
}

/// Changes when the bundle is updated or replaced: the modification times of the bundle,
/// its Contents folder and its Info.plist.
fn stamp(app: &Path) -> Option<String> {
    let t = |p: &Path| -> Option<u128> {
        std::fs::symlink_metadata(p)
            .ok()?
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_nanos())
    };
    Some(format!(
        "{}-{}-{}",
        t(app)?,
        t(&app.join("Contents"))?,
        t(&app.join("Contents/Info.plist"))?
    ))
}

/// Places an app keeps its data, by bundle id (`{id}`) or by name (`{name}`). Each pattern
/// is one path; a `*` stays within one component.
const LEFTOVERS: &[(&str, &str)] = &[
    ("~/Library/Application Support/{id}", "Application data"),
    ("~/Library/Caches/{id}", "Cache"),
    ("~/Library/Containers/{id}", "Sandbox container"),
    ("~/Library/Group Containers/*.{id}", "Shared container"),
    ("~/Library/Group Containers/group.{id}", "Shared container"),
    ("~/Library/Preferences/{id}.plist", "Preferences"),
    ("~/Library/Preferences/{id}.*.plist", "Preferences"),
    (
        "~/Library/Saved Application State/{id}.savedState",
        "Window state",
    ),
    ("~/Library/HTTPStorages/{id}", "Web storage"),
    ("~/Library/HTTPStorages/{id}.binarycookies", "Web storage"),
    ("~/Library/WebKit/{id}", "Web data"),
    ("~/Library/Cookies/{id}.binarycookies", "Cookies"),
    ("~/Library/Application Scripts/{id}", "Scripts"),
    ("~/Library/LaunchAgents/{id}.plist", "Login item"),
    ("~/Library/LaunchAgents/{id}.*.plist", "Login item"),
    ("~/Library/Logs/{id}", "Logs"),
    ("~/Library/Application Support/{name}", "Application data"),
    ("~/Library/Caches/{name}", "Cache"),
    ("~/Library/Logs/{name}", "Logs"),
];

/// The app bundle (to the Trash) and everything it left in the Library, as picks.
pub fn uninstall_picks(app: &App, ctx: &Context, cancel: &AtomicBool) -> Vec<Pick> {
    let mut picks = Vec::new();
    let root = app.path.parent().unwrap_or(Path::new("/")).to_path_buf();
    let mut bundle = Pick::new(app.path.clone(), root, Purpose::Uninstall, Action::Trash);
    bundle.group = app.name.clone();
    bundle.detail = "App".into();
    bundle.preselect = true;
    bundle.running = vec![app.bundle_id.clone()];
    picks.push(bundle);

    // a name that is too short or generic could be another app's folder
    let name_usable = app.name.chars().filter(|c| c.is_alphanumeric()).count() >= 4;
    let mut seen = std::collections::HashSet::new();
    for (pattern, what) in LEFTOVERS {
        if pattern.contains("{name}") && !name_usable {
            continue;
        }
        let pattern = pattern
            .replace("{id}", &app.bundle_id)
            .replace("{name}", &app.name);
        if pattern.contains("/../") || pattern.contains("/./") {
            continue;
        }
        for path in rules::expand(&pattern, ctx) {
            if !seen.insert(path.clone()) {
                continue;
            }
            let root = path.parent().unwrap_or(&ctx.home).to_path_buf();
            let mut pick = Pick::new(path, root, Purpose::Uninstall, Action::Trash);
            pick.group = app.name.clone();
            pick.detail = (*what).into();
            pick.preselect = true;
            pick.running = vec![app.bundle_id.clone()];
            picks.push(pick);
        }
    }
    let paths: Vec<PathBuf> = picks.iter().map(|p| p.path.clone()).collect();
    for (p, b) in picks
        .iter_mut()
        .zip(size::allocated_many(&paths, cancel, &mut |_| {}))
    {
        p.bytes = b;
    }
    picks
}

/// Starts the vendor's uninstaller for the app registered under `key` (Windows). It runs
/// on its own, with its own window; OpenCleaner does not wait for it.
#[cfg(windows)]
pub fn run_uninstaller(key: &str) -> Result<String, String> {
    use std::os::windows::process::CommandExt;
    let app = win::list()
        .into_iter()
        .find(|a| a.bundle_id == key)
        .ok_or("this app is no longer installed")?;
    let command = app.uninstall.clone().ok_or("this app has no uninstaller")?;
    std::process::Command::new("cmd.exe")
        .arg("/C")
        .raw_arg(&command)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW: no console behind the uninstaller
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(app.name)
}

#[cfg(not(windows))]
pub fn run_uninstaller(_key: &str) -> Result<String, String> {
    Err("only on Windows; macOS apps go to the Trash".into())
}

/// Windows: the apps listed under Programs and Features (Uninstall keys of the machine, in
/// both registry views, and of the user).
#[cfg(windows)]
mod win {
    use super::App;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY,
        RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW,
    };

    const UNINSTALL: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    fn string(key: HKEY, sub: &[u16], name: &str) -> Option<String> {
        let name = wide(name);
        let mut len = 0u32;
        let probe = unsafe {
            RegGetValueW(
                key,
                sub.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut len,
            )
        };
        if probe != ERROR_SUCCESS || len < 2 {
            return None;
        }
        let mut buf = vec![0u16; len as usize / 2 + 1];
        let mut bytes = (buf.len() * 2) as u32;
        let rc = unsafe {
            RegGetValueW(
                key,
                sub.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if rc != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let s = String::from_utf16_lossy(&buf[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    fn dword(key: HKEY, sub: &[u16], name: &str) -> Option<u32> {
        let name = wide(name);
        let mut v = 0u32;
        let mut len = 4u32;
        let rc = unsafe {
            RegGetValueW(
                key,
                sub.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut v as *mut u32).cast(),
                &mut len,
            )
        };
        (rc == ERROR_SUCCESS).then_some(v)
    }

    pub fn list() -> Vec<App> {
        let mut apps: Vec<App> = Vec::new();
        for (root, view) in [
            (HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
            (HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
            (HKEY_CURRENT_USER, 0),
        ] {
            let mut key: HKEY = std::ptr::null_mut();
            let path = wide(UNINSTALL);
            if unsafe { RegOpenKeyExW(root, path.as_ptr(), 0, KEY_READ | view, &mut key) }
                != ERROR_SUCCESS
            {
                continue;
            }
            let mut index = 0u32;
            loop {
                let mut name = [0u16; 256];
                let mut len = name.len() as u32;
                let rc = unsafe {
                    RegEnumKeyExW(
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
                let sub: Vec<u16> = name[..len as usize].iter().copied().chain([0]).collect();
                let key_name = String::from_utf16_lossy(&name[..len as usize]);
                // updates, parts of other products and system components are not apps to remove
                if dword(key, &sub, "SystemComponent") == Some(1)
                    || string(key, &sub, "ParentKeyName").is_some()
                    || string(key, &sub, "ReleaseType")
                        .is_some_and(|t| t.contains("Update") || t.contains("Hotfix"))
                {
                    continue;
                }
                let Some(display) = string(key, &sub, "DisplayName") else {
                    continue;
                };
                let Some(uninstall) = string(key, &sub, "UninstallString")
                    .or_else(|| string(key, &sub, "QuietUninstallString"))
                else {
                    continue;
                };
                let location = string(key, &sub, "InstallLocation")
                    .map(|l| PathBuf::from(l.trim_matches('"')))
                    .unwrap_or_default();
                let app = App {
                    name: display,
                    bundle_id: key_name,
                    version: string(key, &sub, "DisplayVersion").unwrap_or_default(),
                    path: if location.is_dir() {
                        location
                    } else {
                        PathBuf::new()
                    },
                    running: false,
                    publisher: string(key, &sub, "Publisher"),
                    estimated_size: dword(key, &sub, "EstimatedSize")
                        .map(|kb| u64::from(kb) * 1024),
                    uninstall: Some(uninstall),
                };
                if !apps
                    .iter()
                    .any(|a| a.name == app.name && a.version == app.version)
                {
                    apps.push(app);
                }
            }
            unsafe { RegCloseKey(key) };
        }
        apps.sort_by_key(|a| a.name.to_lowercase());
        apps
    }
}
