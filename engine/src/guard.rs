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
            Purpose::Files => matches!(
                *p,
                "Documents" | "Desktop" | "Downloads" | "Pictures" | "Movies" | "Music" | "Videos"
            ),
        };
        if !open && is_within(rel, Path::new(p)) {
            return Err("personal files");
        }
    }
    if KEEP_ITSELF.iter().any(|k| rel == Path::new(k)) {
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
        Purpose::Files if !std::fs::symlink_metadata(item).is_ok_and(|m| m.is_file()) => {
            return Err("not a plain file");
        }
        _ => {}
    }
    no_symlink_between(&ctx.home, item)?;
    Ok(())
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

/// A child the user ticked inside a listed folder: it must sit directly in that folder,
/// and the folder must still be the one that was listed. Returns the child's identity.
pub fn part_of(parent: &Path, parent_id: &Identity, part: &Path) -> Result<Identity, String> {
    if part.parent() != Some(parent) {
        return Err("not inside the listed folder".into());
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

/// The path still names the object that was measured (not replaced, not a new link).
pub fn still_same(path: &Path, was: &Identity) -> bool {
    identity(path).is_some_and(|now| now == *was)
}
