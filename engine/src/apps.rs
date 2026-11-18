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

#[derive(Clone, Debug, Serialize)]
pub struct App {
    pub name: String,
    pub bundle_id: String,
    pub version: String,
    pub path: PathBuf,
    pub running: bool,
}

/// Apps that come with the system, or that are OpenCleaner itself.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const KEEP: &[&str] = &["com.apple.Safari", "io.github.opencleaner"];

/// Installed apps, by name. Sizes come separately (`sizes`), since reading them takes a
/// walk of every bundle.
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
    })
}

#[cfg(not(target_os = "macos"))]
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
