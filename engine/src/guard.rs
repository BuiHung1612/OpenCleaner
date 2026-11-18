//! What may be removed. Every item passes these checks when the plan is made and again
//! right before it is removed; a failed check means skip, never "try anyway".

use crate::context::Context;
use crate::fs::{self, Detail, Kind};
use crate::rules::{is_clean_path, is_within};
use std::path::{Path, PathBuf};

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

/// May `item` (found from `root`) be removed? Checks only the path; `still_same` checks
/// that it is still the file that was measured.
pub fn check(item: &Path, root: &Path, ctx: &Context) -> Result<(), Refusal> {
    if !is_clean_path(item) {
        return Err("not a plain absolute path");
    }
    // only the user's own files: never the system, other users or other volumes
    if !is_within(item, &ctx.home) || item == ctx.home {
        return Err("outside the home folder");
    }
    let rel = item.strip_prefix(&ctx.home).unwrap();
    for p in PRIVATE {
        if is_within(rel, Path::new(p)) {
            return Err("personal files");
        }
    }
    if KEEP_ITSELF.iter().any(|k| rel == Path::new(k)) {
        return Err("a folder that is kept");
    }
    if !is_within(item, root) {
        return Err("outside the rule's folder");
    }
    no_symlink_between(&ctx.home, item)?;
    Ok(())
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

/// The path still names the object that was measured (not replaced, not a new link).
pub fn still_same(path: &Path, was: &Identity) -> bool {
    identity(path).is_some_and(|now| now == *was)
}
