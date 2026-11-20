//! What may be removed. Every item passes these checks when the plan is made and again
//! right before it is removed; a failed check means skip, never "try anyway".

use crate::context::Context;
use crate::fs::{self, Detail, Kind};
use crate::rules::{is_clean_path, is_within, is_within_nocase};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Why an item is refused.
pub type Refusal = &'static str;

/// Folders whose contents are the user's own data: nothing inside is ever removed.
#[cfg(target_os = "macos")]
const PRIVATE: &[&str] = &[
    "Documents",
    "Desktop",
    "Pictures",
    "Movies",
    "Music",
    "Public",
    "Downloads",
    ".ssh",
    ".gnupg",
    "Library/Mobile Documents",
    "Library/CloudStorage",
    "Library/Keychains",
    "Library/Mail",
    "Library/Messages",
    "Library/Photos",
    "Library/Preferences",
    "Library/Accounts",
    "Library/Cookies",
    "Library/Calendars",
    "Library/Safari",
];

#[cfg(windows)]
const PRIVATE: &[&str] = &[
    "Documents",
    "Desktop",
    "Pictures",
    "Videos",
    "Music",
    "Downloads",
    "OneDrive",
    ".ssh",
    "AppData/Roaming/Microsoft/Credentials",
    "AppData/Roaming/Microsoft/Protect",
    "AppData/Local/Microsoft/Credentials",
];

#[cfg(not(any(target_os = "macos", windows)))]
const PRIVATE: &[&str] = &["Documents", "Desktop", "Pictures", "Downloads", ".ssh"];

/// Folders that may hold removable things but are never removed themselves.
const KEEP_ITSELF: &[&str] = &[
    "Library",
    "Library/Caches",
    "Library/Logs",
    "Library/Application Support",
    "Library/Containers",
    "Library/Group Containers",
    "Library/Developer",
    ".Trash",
    ".cache",
    ".config",
    ".local",
    "AppData",
    "AppData/Local",
    "AppData/Roaming",
    "AppData/LocalLow",
    "AppData/Local/Temp",
];

/// What the removal is for. Each purpose opens only what it needs on top of the rules
/// shared by all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// System Junk: caches and logs.
    Clean,
    /// An app and its leftovers: also its preferences and cookies, and the bundle itself
    /// in /Applications.
    Uninstall,
    /// Installer files (.dmg, .pkg…) in Downloads and on the Desktop.
    Installers,
    /// Rebuildable build folders inside projects, wherever the projects live (Documents too).
    Projects,
    /// Hidden items directly in the home folder (`.android`, old `.zshrc` backups), never
    /// the ones that hold settings or keys.
    HomeItems,
    /// The user's own files (large and old, duplicates, screenshots): single files in their
    /// folders, to the Trash only.
    Files,
}

/// Extensions of installer files the Installers tool may move to the Trash.
pub const INSTALLER_EXTENSIONS: &[&str] =
    &["dmg", "pkg", "mpkg", "iso", "xip", "msi", "msix", "appx"];

/// Names of the build folders the Projects tool may remove (with a marker next to them,
/// checked by the finder).
pub const PROJECT_ARTIFACTS: &[&str] = &[
    "node_modules",
    "target",
    "build",
    ".dart_tool",
    ".gradle",
    "dist",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    "Pods",
    ".venv",
    "venv",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
];

/// May `item` (found from `root`) be removed by System Junk?
pub fn check(item: &Path, root: &Path, ctx: &Context) -> Result<(), Refusal> {
    check_for(item, root, ctx, Purpose::Clean)
}

/// May `item` (found from `root`) be removed for `purpose`? Checks only the path;
/// `still_same` checks that it is still the object that was measured.
pub fn check_for(item: &Path, root: &Path, ctx: &Context, purpose: Purpose) -> Result<(), Refusal> {
    if !is_clean_path(item) {
        return Err("not a plain absolute path");
    }
    if let Some(why) = exclusion_refusal(item, ctx) {
        return Err(why);
    }
    let name = item
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // a macOS installer app ("Install macOS Sequoia.app") left in /Applications
    if purpose == Purpose::Installers && is_app_in_applications(item) {
        if !name.starts_with("Install macOS") || item.parent() != Some(Path::new(APPLICATIONS)) {
            return Err("not a macOS installer");
        }
        return no_symlink_between(Path::new(APPLICATIONS), item);
    }
    if purpose == Purpose::Uninstall && is_app_in_applications(item) {
        if !is_within(item, root) {
            return Err("outside the app's folder");
        }
        return no_symlink_between(Path::new(APPLICATIONS), item);
    }
    // otherwise only the user's own files: never the system, other users or other volumes
    if !is_within(item, &ctx.home) || item == ctx.home {
        return Err("outside the home folder");
    }
    let rel = item.strip_prefix(&ctx.home).unwrap();
    for p in PRIVATE {
        let open = match purpose {
            Purpose::Clean => false,
            Purpose::Uninstall => matches!(*p, "Library/Preferences" | "Library/Cookies"),
            Purpose::Installers => matches!(*p, "Downloads" | "Desktop"),
            Purpose::Projects => matches!(*p, "Documents" | "Desktop"),
            Purpose::HomeItems => false,
            Purpose::Files => matches!(
                *p,
                "Documents" | "Desktop" | "Downloads" | "Pictures" | "Movies" | "Music" | "Videos"
            ),
        };
        if !open && is_within_nocase(rel, Path::new(p)) {
            return Err("personal files");
        }
    }
    if KEEP_ITSELF
        .iter()
        .any(|k| is_within_nocase(rel, Path::new(k)) && is_within_nocase(Path::new(k), rel))
    {
        return Err("a folder that is kept");
    }
    if !is_within(item, root) {
        return Err("outside the rule's folder");
    }
    match purpose {
        Purpose::Installers => {
            let ext = item
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if !INSTALLER_EXTENSIONS.contains(&ext.as_str()) {
                return Err("not an installer file");
            }
        }
        Purpose::Projects if !PROJECT_ARTIFACTS.contains(&name.as_str()) => {
            return Err("not a build folder");
        }
        Purpose::HomeItems
            if item.parent() != Some(ctx.home.as_path())
                || !name.starts_with('.')
                || !crate::dotfiles::removable(
                    &name,
                    std::fs::symlink_metadata(item).is_ok_and(|m| m.is_dir()),
                ) =>
        {
            return Err("not a hidden item of the home folder");
        }
        Purpose::Files if !std::fs::symlink_metadata(item).is_ok_and(|m| m.is_file()) => {
            return Err("not a plain file");
        }
        _ => {}
    }
    no_symlink_between(&ctx.home, item)?;
    Ok(())
}

/// The folders the user excluded in Settings, read once per data folder (tests each have
/// their own) and kept until `set_exclusions` replaces them. `Err`: the file is there but
/// cannot be read, and then nothing may be removed (an empty list would protect nothing).
type Loaded = Result<Vec<PathBuf>, Refusal>;
static EXCLUDED: Mutex<Vec<(PathBuf, Loaded)>> = Mutex::new(Vec::new());

/// Why every removal is refused while the exclusions file is damaged.
pub const UNREADABLE: Refusal = "exclusions could not be read";

fn exclusions_file(ctx: &Context) -> PathBuf {
    ctx.data_dir.join("exclusions.json")
}

/// Folders never scanned into or removed from; `Err` when the saved list cannot be read
/// (no file at all is an empty list).
pub fn load_exclusions(ctx: &Context) -> Loaded {
    let mut cache = EXCLUDED.lock().unwrap();
    if let Some((_, list)) = cache.iter().find(|(d, _)| *d == ctx.data_dir) {
        return list.clone();
    }
    let list = match std::fs::read(exclusions_file(ctx)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(UNREADABLE),
        Ok(b) => serde_json::from_slice(&b).map_err(|_| UNREADABLE),
    };
    cache.push((ctx.data_dir.clone(), list.clone()));
    list
}

/// Folders never scanned into (empty when the list cannot be read: every removal is
/// refused then, see `exclusion_refusal`).
pub fn exclusions(ctx: &Context) -> Vec<PathBuf> {
    load_exclusions(ctx).unwrap_or_default()
}

/// Saves the list atomically (a crash leaves the old file or the new one, never half of
/// one); only plain absolute paths are kept.
pub fn set_exclusions(ctx: &Context, paths: Vec<PathBuf>) -> std::io::Result<()> {
    let mut list: Vec<PathBuf> = paths.into_iter().filter(|p| is_clean_path(p)).collect();
    list.sort();
    list.dedup();
    std::fs::create_dir_all(&ctx.data_dir)?;
    let tmp = ctx
        .data_dir
        .join(format!(".exclusions-{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&list)?)?;
    if let Err(e) = std::fs::rename(&tmp, exclusions_file(ctx)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let mut cache = EXCLUDED.lock().unwrap();
    cache.retain(|(d, _)| *d != ctx.data_dir);
    cache.push((ctx.data_dir.clone(), Ok(list)));
    Ok(())
}

/// Not to be scanned into: inside (or is) a folder the user excluded, or the list cannot
/// be read. (A folder that only holds one is scanned; removing it is refused.)
pub fn excluded(item: &Path, ctx: &Context) -> bool {
    exclusion_refusal(item, ctx).is_some_and(|why| why != HOLDS)
}

const HOLDS: Refusal = "holds a folder excluded in Settings";

/// Why the exclusions forbid touching `item`: it is in an excluded folder, it holds one
/// (removing it would take the excluded folder along), or the list cannot be read.
///
/// Names are compared without case where the file system ignores it (macOS, Windows).
/// Another spelling of the same folder (Unicode form, a link or firmlink such as
/// /System/Volumes/Data) is caught by identity: walking up from the item meets an excluded
/// folder's (dev, ino), or walking up from an excluded folder meets the item's. The item's
/// folders are read without following links, so callers pass a path whose folders are
/// real (check_for refuses a link in the path; Space Lens resolves the parent first).
// ponytail: a few stats per item per exclusion; cache the excluded folders' ancestry if
// scans with many exclusions get slow.
pub fn exclusion_refusal(item: &Path, ctx: &Context) -> Option<Refusal> {
    const INSIDE: Refusal = "excluded in Settings";
    let list = match load_exclusions(ctx) {
        Ok(l) => l,
        Err(why) => return Some(why),
    };
    if list.is_empty() {
        return None;
    }
    for e in &list {
        if is_within_nocase(item, e) {
            return Some(INSIDE);
        }
        if is_within_nocase(e, item) {
            return Some(HOLDS);
        }
    }
    let key = |i: Identity| (i.ino != 0).then_some((i.dev, i.ino));
    let ids_up = |p: &Path| -> Vec<(u64, u64)> {
        p.ancestors().filter_map(identity).filter_map(key).collect()
    };
    let item_up = ids_up(item);
    let item_id = identity(item).and_then(key);
    for e in &list {
        let Ok(real) = std::fs::canonicalize(e) else {
            continue; // not there: nothing on disk to protect under another name
        };
        if identity(&real)
            .and_then(key)
            .is_some_and(|id| item_up.contains(&id))
        {
            return Some(INSIDE);
        }
        if item_id.is_some_and(|id| ids_up(&real).contains(&id)) {
            return Some(HOLDS);
        }
    }
    None
}

/// Space Lens: may the user's pick `path` go to the Trash? Only the exclusions apply (the
/// user points at it), checked on the path as given and on where it really is (its parent
/// resolved: links, firmlinks, case). Returns the resolved path, which is what is moved.
pub fn check_trash_path(path: &Path, ctx: &Context) -> Result<PathBuf, Refusal> {
    if !is_clean_path(path) {
        return Err("not a plain absolute path");
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err("no parent folder");
    };
    let real = std::fs::canonicalize(parent)
        .map_err(|_| "the folder is gone")?
        .join(name);
    if std::fs::symlink_metadata(&real).is_err() {
        return Err("no longer there");
    }
    for p in [path, real.as_path()] {
        if let Some(why) = exclusion_refusal(p, ctx) {
            return Err(why);
        }
    }
    Ok(real)
}

#[cfg(target_os = "macos")]
const APPLICATIONS: &str = "/Applications";
#[cfg(not(target_os = "macos"))]
const APPLICATIONS: &str = "/nonexistent-applications";

/// /Applications/X.app or /Applications/<folder>/X.app.
fn is_app_in_applications(item: &Path) -> bool {
    let Ok(rel) = item.strip_prefix(APPLICATIONS) else {
        return false;
    };
    let depth = rel.components().count();
    (1..=2).contains(&depth) && item.extension().is_some_and(|e| e == "app")
}

/// Every directory from `base` down to the item's parent must be a real directory: a
/// symlink there could point the removal anywhere.
fn no_symlink_between(base: &Path, item: &Path) -> Result<(), Refusal> {
    let Some(parent) = item.parent() else {
        return Err("no parent folder");
    };
    let Ok(rel) = parent.strip_prefix(base) else {
        return Err("outside the home folder");
    };
    let mut cur = PathBuf::from(base);
    for c in rel.components() {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Err("a link in the path"),
            Ok(m) if !m.is_dir() => return Err("a file in the path"),
            Ok(_) => {}
            Err(_) => return Err("the folder is gone"),
        }
    }
    Ok(())
}

/// What identifies a file system object between the scan and the removal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub dev: u64,
    pub ino: u64,
    pub kind: Kind,
}

pub fn identity(path: &Path) -> Option<Identity> {
    let e = fs::stat(path, Detail::Basic).ok()?;
    Some(Identity {
        dev: e.dev,
        ino: e.ino,
        kind: e.kind,
    })
}

/// Something the user ticked inside a listed folder, at any depth: it must sit strictly
/// inside that folder (a plain path, no `.`/`..`), every folder between them must be a
/// real directory (not a link), and the listed folder must still be the one that was
/// listed. Returns the part's identity.
pub fn part_of(parent: &Path, parent_id: &Identity, part: &Path) -> Result<Identity, String> {
    if !is_clean_path(part) || part == parent || !is_within(part, parent) {
        return Err("not inside the listed folder".into());
    }
    let mut dir = part.parent();
    while let Some(d) = dir.filter(|d| *d != parent) {
        // lstat: a link here could point the removal anywhere
        if !std::fs::symlink_metadata(d).is_ok_and(|m| m.is_dir()) {
            return Err("a link or a file in the path".into());
        }
        dir = d.parent();
    }
    if !still_same(parent, parent_id) {
        return Err("changed since it was listed".into());
    }
    identity(part).ok_or_else(|| "no longer there".into())
}

/// What removing `path` frees, measured now (for parts, which were not measured before).
pub fn freed_now(path: &Path) -> u64 {
    let opts = crate::size::Options {
        mode: crate::size::Mode::Freed,
        ..Default::default()
    };
    crate::size::measure(path, &opts).map_or(0, |r| r.bytes)
}

/// An object as it was at one moment: identity plus length and modification time. Kept in
/// History for what went to the Trash (only that very item is put back) and for the
/// user's files (a file edited since it was listed is not removed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp {
    pub dev: u64,
    pub ino: u64,
    pub len: u64,
    pub mtime: i64,
}

pub fn stamp(path: &Path) -> Option<Stamp> {
    let e = fs::stat(path, Detail::Basic).ok()?;
    Some(Stamp {
        dev: e.dev,
        ino: e.ino,
        len: e.len,
        mtime: e.mtime,
    })
}

/// The path still names the object that was measured (not replaced, not a new link).
pub fn still_same(path: &Path, was: &Identity) -> bool {
    identity(path).is_some_and(|now| now == *was)
}
