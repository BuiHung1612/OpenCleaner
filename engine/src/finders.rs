//! Installers and Project Junk: find candidates, measure them, hand them out as picks.

use crate::context::Context;
use crate::fs::walk::{self, Visitor};
use crate::fs::{self, Detail, Entry, Kind};
use crate::guard::{INSTALLER_EXTENSIONS, Purpose};
use crate::picks::{Action, Pick};
use crate::size;
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

const DAY: u64 = 86_400;

fn age_days(mtime: i64) -> u64 {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.saturating_sub(mtime.max(0) as u64) / DAY
}

/// Installer files (.dmg, .pkg, .iso…) in Downloads and on the Desktop, two levels deep.
/// The ones older than a week start selected.
pub fn installers(ctx: &Context) -> Vec<Pick> {
    let mut out = Vec::new();
    for folder in ["Downloads", "Desktop"] {
        let root = ctx.home.join(folder);
        collect_installers(&root, &root, 0, folder, &mut out);
    }
    #[cfg(target_os = "macos")]
    macos_installers(&mut out);
    out.sort_by_key(|p| std::cmp::Reverse(p.bytes));
    out
}

/// "Install macOS ….app" left in /Applications after an upgrade (12 GB or more each).
#[cfg(target_os = "macos")]
fn macos_installers(out: &mut Vec<Pick>) {
    let apps = Path::new("/Applications");
    let Ok(entries) = std::fs::read_dir(apps) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !(name.starts_with("Install macOS") && name.ends_with(".app")) {
            continue;
        }
        let path = entry.path();
        let mut pick = Pick::new(
            path.clone(),
            apps.to_path_buf(),
            Purpose::Installers,
            Action::Trash,
        );
        pick.bytes = size::freed_many(&[path], &AtomicBool::new(false), &mut |_| {})[0];
        pick.group = "Applications".into();
        pick.detail = "macOS installer · downloaded again from the App Store when needed".into();
        pick.preselect = true;
        out.push(pick);
    }
}

fn collect_installers(root: &Path, dir: &Path, depth: u32, group: &str, out: &mut Vec<Pick>) {
    let mut subdirs = Vec::new();
    let _ = fs::read_dir(dir, Detail::Basic, &mut |e: &Entry| {
        let path = dir.join(&e.name);
        match e.kind {
            Kind::Dir if depth < 1 && !e.name.to_string_lossy().ends_with(".app") => {
                subdirs.push(path)
            }
            Kind::File => {
                let ext = path
                    .extension()
                    .map(|x| x.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if INSTALLER_EXTENSIONS.contains(&ext.as_str()) {
                    let age = age_days(e.mtime);
                    let mut pick =
                        Pick::new(path, root.to_path_buf(), Purpose::Installers, Action::Trash);
                    pick.bytes = e.alloc;
                    pick.group = group.into();
                    pick.kind = ext.to_ascii_uppercase();
                    pick.age_days = Some(age as i64);
                    pick.detail = format!("{} · {age} days old", pick.kind);
                    pick.preselect = age >= 7;
                    out.push(pick);
                }
            }
            _ => {}
        }
    });
    for sub in subdirs {
        collect_installers(root, &sub, depth + 1, group, out);
    }
}

/// Where projects usually live; the ones that exist are searched (each folder once, even
/// when two names reach it on a case-insensitive disk).
const PROJECT_ROOTS: &[&str] = &[
    "Projects",
    "projects",
    "Developer",
    "dev",
    "Code",
    "code",
    "src",
    "workspace",
    "Workspace",
    "GitHub",
    "github",
    "repos",
    "Sites",
    "Documents/GitHub",
    "Documents/Projects",
    "Documents/Code",
];

/// Rebuildable build folders inside projects. A folder counts only with the project file
/// that makes it rebuildable next to it; folders holding git-tracked files are left out;
/// projects touched in the last week are listed but not preselected.
pub fn projects(ctx: &Context, extra_roots: &[PathBuf], cancel: &AtomicBool) -> Vec<Pick> {
    let mut seen = HashSet::new();
    let mut roots = Vec::new();
    for r in PROJECT_ROOTS
        .iter()
        .map(|r| ctx.home.join(r))
        .chain(extra_roots.iter().cloned())
    {
        if let Ok(e) = fs::stat(&r, Detail::Basic)
            && e.kind == Kind::Dir
            && seen.insert((e.dev, e.ino))
        {
            roots.push(r);
        }
    }
    let finder = ArtifactFinder {
        found: Mutex::new(Vec::new()),
        roots: roots.clone(),
    };
    walk::walk_tagged(
        roots
            .iter()
            .enumerate()
            .map(|(i, r)| (r.clone(), i as u32))
            .collect(),
        walk::default_threads(),
        Detail::Basic,
        cancel,
        &finder,
    );
    let git = git_binary();
    let mut picks = Vec::new();
    for (artifact, project, kind, root) in finder.found.into_inner().unwrap() {
        let name = artifact
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut pick = Pick::new(artifact.clone(), root, Purpose::Projects, Action::Delete);
        pick.group = project
            .strip_prefix(&ctx.home)
            .map(|p| format!("~/{}", p.display()))
            .unwrap_or_else(|_| project.display().to_string());
        pick.detail = kind.into();
        pick.kind = kind.into();
        pick.preselect = true;
        match in_git_repo(&project).then(|| tracked(git.as_deref(), &project, &name)) {
            Some(Some(true)) => continue, // holds files under version control: not junk
            Some(None) => {
                pick.preselect = false;
                pick.detail.push_str(" · git status unknown");
                pick.flags.push("git_unknown");
            }
            _ => {}
        }
        if recently_active(&project, &artifact) {
            pick.preselect = false;
            pick.detail.push_str(" · active this week");
            pick.flags.push("active");
        }
        picks.push(pick);
    }
    let paths: Vec<PathBuf> = picks.iter().map(|p| p.path.clone()).collect();
    let sizes = size::freed_many(&paths, cancel, &mut |_| {});
    for (p, b) in picks.iter_mut().zip(sizes) {
        p.bytes = b;
    }
    picks.retain(|p| p.bytes > 0);
    picks.sort_by_key(|p| std::cmp::Reverse(p.bytes));
    picks
}

/// (artifact, project folder, what it is, search root)
type Found = (PathBuf, PathBuf, &'static str, PathBuf);

struct ArtifactFinder {
    found: Mutex<Vec<Found>>,
    roots: Vec<PathBuf>,
}

impl Visitor for ArtifactFinder {
    fn entry(&self, dir: &Path, tag: u32, e: &Entry) -> Option<u32> {
        if e.kind != Kind::Dir || e.mount_point {
            return None;
        }
        let root = &self.roots[tag as usize];
        let depth = dir
            .components()
            .count()
            .saturating_sub(root.components().count());
        let name = e.name.to_string_lossy();
        // a folder with its own .git (dir or file) is a repository someone cloned there,
        // not build output: never listed, looked into like any folder
        if let Some(kind) = artifact_kind(&name, dir)
            && std::fs::symlink_metadata(dir.join(&e.name).join(".git")).is_err()
        {
            self.found.lock().unwrap().push((
                dir.join(&e.name),
                dir.to_path_buf(),
                kind,
                root.clone(),
            ));
            return None;
        }
        // not into version control internals, other hidden folders, app bundles, or too deep
        if depth >= 6 || name.starts_with('.') || name.ends_with(".app") || name == "Library" {
            return None;
        }
        Some(tag)
    }

    fn error(&self, _path: &Path, _tag: u32, _err: io::Error) {}
}

/// What a build folder named `name` in `project` is, if the project file that makes it
/// rebuildable is there.
fn artifact_kind(name: &str, project: &Path) -> Option<&'static str> {
    let has = |f: &str| project.join(f).exists();
    let any = |fs: &[&str]| fs.iter().any(|f| has(f));
    match name {
        "node_modules" if has("package.json") => Some("npm packages"),
        "target" if has("Cargo.toml") => Some("Rust build"),
        "target" if has("pom.xml") => Some("Maven build"),
        "build" if any(&["build.gradle", "build.gradle.kts"]) => Some("Gradle build"),
        "build" if has("pubspec.yaml") => Some("Flutter build"),
        "build" if has("CMakeLists.txt") => Some("CMake build"),
        ".dart_tool" if has("pubspec.yaml") => Some("Dart tool cache"),
        ".gradle"
            if any(&[
                "settings.gradle",
                "settings.gradle.kts",
                "build.gradle",
                "build.gradle.kts",
            ]) =>
        {
            Some("Gradle project cache")
        }
        "dist" if has("package.json") => Some("Build output"),
        ".next" | ".nuxt" | ".svelte-kit" | ".turbo" | ".parcel-cache" if has("package.json") => {
            Some("Web build cache")
        }
        "Pods" if has("Podfile") => Some("CocoaPods"),
        ".venv" | "venv" if project.join(name).join("pyvenv.cfg").exists() => {
            Some("Python environment")
        }
        ".pytest_cache" | ".mypy_cache" | ".ruff_cache" | ".tox"
            if any(&[
                "pyproject.toml",
                "setup.py",
                "setup.cfg",
                "tox.ini",
                "requirements.txt",
            ]) =>
        {
            Some("Python tool cache")
        }
        _ => None,
    }
}

fn in_git_repo(project: &Path) -> bool {
    project.ancestors().any(|a| a.join(".git").exists())
}

/// A git that will not pop up an "install developer tools" dialog.
fn git_binary() -> Option<String> {
    if cfg!(windows) {
        return Some("git".into());
    }
    for p in ["/opt/homebrew/bin/git", "/usr/local/bin/git"] {
        if Path::new(p).exists() {
            return Some(p.into());
        }
    }
    let tools = Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .output()
        .ok()?;
    tools.status.success().then(|| "/usr/bin/git".into())
}

/// Does `name` in `project` hold files under version control? None when git cannot say.
fn tracked(git: Option<&str>, project: &Path, name: &str) -> Option<bool> {
    let out = Command::new(git?)
        .arg("-C")
        .arg(project)
        .args(["ls-files", "-z", "--"])
        .arg(name)
        .output()
        .ok()?;
    out.status.success().then_some(!out.stdout.is_empty())
}

/// The project or its build folder changed in the last 7 days.
fn recently_active(project: &Path, artifact: &Path) -> bool {
    let week = Duration::from_secs(7 * DAY);
    [project, artifact].iter().any(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .is_ok_and(|t| {
                SystemTime::now()
                    .duration_since(t)
                    .is_ok_and(|age| age < week)
            })
    })
}
