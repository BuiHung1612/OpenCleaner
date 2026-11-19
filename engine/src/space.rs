//! Space Lens: what takes up space in a folder, or on the startup disk.
//!
//! One parallel walk sizes every child at once (each queued directory is tagged with the
//! child it belongs to), so the work spreads over all workers however uneven the
//! children are, and partial sizes can be shown while it runs: about every 200 ms a
//! `progress` listing carries each child's size so far, "scanning" until its whole
//! subtree is read, then "complete" (or "partial").
//!
//! Repeat scans of unchanged directories come from `dircache` (see there for what that
//! can miss and for how long).

use crate::dircache::{self, Rec};
use crate::fs::walk::{self, Visitor};
use crate::fs::{self, Detail, Entry, Kind};
use crate::size::Links;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub files: u64,
    pub is_dir: bool,
    /// "complete"; "partial" when part of it could not be read (size is a lower bound);
    /// "scanning" in a `progress` listing while its size is still growing
    pub scan_status: &'static str,
    /// A real item the user may act on. Overview rows (Home, Applications, System &
    /// other…) are summaries and must never be offered for removal.
    pub selectable: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Listing {
    pub path: Option<PathBuf>,
    pub overview: bool,
    pub entries: Vec<Item>,
    pub total_size: u64,
    pub total_files: u64,
    pub scan_status: &'static str,
    pub cancelled: bool,
}

/// A part to size: one child of the folder, or one root of the overview.
struct Part {
    name: String,
    path: PathBuf,
    is_dir: bool,
    /// Size known without walking (files, symlinks, mount points)
    fixed: u64,
    walk: bool,
}

/// Sizes the children of `dir`, calling `progress` with partial results while it runs.
/// With `cache` (a directory), unchanged directories are taken from `dircache` there.
pub fn list(
    dir: &Path,
    cache: Option<&Path>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Listing),
) -> io::Result<Listing> {
    let links = Links::default();
    let mut parts = Vec::new();
    fs::read_dir(dir, Detail::Basic, &mut |e: &Entry| {
        let walk = e.kind == Kind::Dir && !e.mount_point && e.error.is_none();
        // a directory's own blocks count; a mount point is another volume (0); a hard-linked
        // file counts once
        let counted =
            walk || (e.kind != Kind::Dir && (e.nlink == 1 || links.first_time(e.dev, e.ino)));
        parts.push(Part {
            name: e.name.to_string_lossy().into_owned(),
            path: dir.join(&e.name),
            is_dir: e.kind == Kind::Dir,
            fixed: if counted { e.alloc } else { 0 },
            walk,
        });
    })?;
    Ok(measure(
        Some(dir.to_path_buf()),
        false,
        parts,
        HashMap::new(),
        links,
        cache,
        cancel,
        progress,
    ))
}

/// The startup disk at a glance: the big user and system areas, plus what the volume
/// uses outside them.
pub fn overview(
    cache: Option<&Path>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Listing),
) -> io::Result<Listing> {
    let mut parts = Vec::new();
    let mut skip: HashMap<PathBuf, Vec<OsString>> = HashMap::new();
    for (name, path) in overview_roots() {
        if fs::stat(&path, Detail::Basic).is_ok_and(|e| e.kind == Kind::Dir) {
            parts.push(Part {
                name,
                path,
                is_dir: true,
                fixed: 0,
                walk: true,
            });
        }
    }
    // a root inside another root (~/Library in ~) is sized once, as its own part
    for p in &parts {
        if let (Some(parent), Some(name)) = (p.path.parent(), p.path.file_name())
            && parts.iter().any(|q| q.path == parent)
        {
            skip.entry(parent.to_path_buf())
                .or_default()
                .push(name.to_os_string());
        }
    }
    let mut listing = measure(
        None,
        true,
        parts,
        skip,
        Links::default(),
        cache,
        cancel,
        progress,
    );
    if let Some(used) = volume_used(Path::new(&root_volume())) {
        let rest = used.saturating_sub(listing.total_size);
        if rest > 0 && !listing.cancelled {
            listing.entries.push(Item {
                name: "System & other".into(),
                path: PathBuf::from(root_volume()),
                size: rest,
                files: 0,
                is_dir: false,
                scan_status: "complete",
                selectable: false,
            });
            listing.total_size += rest;
            sort(&mut listing.entries);
        }
    }
    Ok(listing)
}

#[allow(clippy::too_many_arguments)]
fn measure(
    path: Option<PathBuf>,
    overview: bool,
    parts: Vec<Part>,
    skip: HashMap<PathBuf, Vec<OsString>>,
    links: Links,
    cache: Option<&Path>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Listing),
) -> Listing {
    let mut sizer = PartSizer {
        left: parts
            .iter()
            .map(|p| AtomicU64::new(u64::from(p.walk)))
            .collect(),
        cache: cache.map(dircache::load),
        found: Mutex::new(Vec::new()),
        now: dircache::now(),
        sizes: parts.iter().map(|p| AtomicU64::new(p.fixed)).collect(),
        files: parts
            .iter()
            .map(|p| AtomicU64::new(u64::from(!p.is_dir)))
            .collect(),
        partial: parts.iter().map(|_| AtomicBool::new(false)).collect(),
        links,
        skip,
        errors: Mutex::new(Vec::new()),
    };
    let roots: Vec<(PathBuf, u32)> = parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.walk)
        .map(|(i, p)| (p.path.clone(), i as u32))
        .collect();

    let snapshot = |done: bool| {
        let mut entries: Vec<Item> = parts
            .iter()
            .enumerate()
            .map(|(i, p)| Item {
                name: p.name.clone(),
                path: p.path.clone(),
                size: sizer.sizes[i].load(Ordering::Relaxed),
                files: sizer.files[i].load(Ordering::Relaxed),
                is_dir: p.is_dir,
                scan_status: if sizer.partial[i].load(Ordering::Relaxed)
                    || (done && cancel.load(Ordering::Relaxed))
                {
                    "partial"
                } else if done || sizer.left[i].load(Ordering::Relaxed) == 0 {
                    "complete"
                } else {
                    "scanning"
                },
                selectable: !overview,
            })
            .filter(|item| !overview || item.size > 0)
            .collect();
        sort(&mut entries);
        let total_size = entries.iter().map(|e| e.size).sum();
        let total_files = entries.iter().map(|e| e.files).sum();
        let scan_status = if entries.iter().any(|e| e.scan_status == "partial") {
            "partial"
        } else if entries.iter().any(|e| e.scan_status == "scanning") {
            "scanning"
        } else {
            "complete"
        };
        Listing {
            path: path.clone(),
            overview,
            entries,
            total_size,
            total_files,
            scan_status,
            cancelled: cancel.load(Ordering::Relaxed),
        }
    };

    std::thread::scope(|scope| {
        let walker = scope.spawn(|| {
            walk::walk_tagged(
                roots,
                walk::default_threads(),
                Detail::Basic,
                cancel,
                &sizer,
            )
        });
        let mut last = Instant::now();
        while !walker.is_finished() {
            std::thread::sleep(Duration::from_millis(20));
            if last.elapsed() >= Duration::from_millis(200) {
                progress(&snapshot(false));
                last = Instant::now();
            }
        }
    });
    let listing = snapshot(true);
    if let Some(dir) = cache {
        sizer.cache = None; // the last reference to the map: store updates it in place
        dircache::store(dir, sizer.found.into_inner().unwrap());
    }
    listing
}

/// Largest first; equal sizes by name so the order is stable.
fn sort(items: &mut [Item]) {
    items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
}

thread_local! {
    /// The directory this worker is reading, as it will be cached
    static READING: RefCell<Option<Rec>> = const { RefCell::new(None) };
}

struct PartSizer {
    /// directories of each part queued or being read; 0: the part is fully sized
    left: Vec<AtomicU64>,
    cache: Option<Arc<dircache::Map>>,
    /// directories read in this scan, for the cache
    found: Mutex<Vec<Rec>>,
    now: i64,
    sizes: Vec<AtomicU64>,
    files: Vec<AtomicU64>,
    partial: Vec<AtomicBool>,
    links: Links,
    /// directories that are parts of their own, by parent
    skip: HashMap<PathBuf, Vec<OsString>>,
    errors: Mutex<Vec<String>>,
}

impl PartSizer {
    fn skipped(&self, dir: &Path, name: &std::ffi::OsStr) -> bool {
        !self.skip.is_empty()
            && self
                .skip
                .get(dir)
                .is_some_and(|names| names.iter().any(|n| n == name))
    }

    /// Updates the cache record of the directory being read, if one is kept.
    fn note(&self, f: impl FnOnce(&mut Option<Rec>)) {
        if self.cache.is_some() {
            READING.with_borrow_mut(f);
        }
    }
}

impl Visitor for PartSizer {
    fn known(&self, dir: &Path, tag: u32, subdirs: &mut Vec<(PathBuf, u32)>) -> bool {
        let Some(cache) = &self.cache else {
            return false;
        };
        READING.set(None);
        let Ok(me) = fs::stat(dir, Detail::Basic) else {
            return false; // reading it reports the error
        };
        if me.mount_point {
            return true; // mounted since it was listed: another volume
        }
        if me.kind != Kind::Dir || me.ino == 0 {
            return false;
        }
        let Some(r) = cache
            .get(&(me.dev, me.ino))
            .filter(|r| r.usable(me.mtime, self.now))
        else {
            READING.set(Some(Rec {
                dev: me.dev,
                ino: me.ino,
                mtime: me.mtime,
                at: self.now,
                ..Rec::default()
            }));
            return false;
        };
        let i = tag as usize;
        self.files[i].fetch_add(r.files, Ordering::Relaxed);
        let mut bytes = r.bytes;
        for &(dev, ino, alloc) in &r.links {
            if self.links.first_time(dev, ino) {
                bytes += alloc;
            }
        }
        for (name, alloc) in &r.dirs {
            if self.skipped(dir, name.as_ref()) {
                continue;
            }
            bytes += alloc;
            self.left[i].fetch_add(1, Ordering::Relaxed);
            subdirs.push((dir.join(name), tag));
        }
        self.sizes[i].fetch_add(bytes, Ordering::Relaxed);
        true
    }

    fn entry(&self, dir: &Path, tag: u32, e: &Entry) -> Option<u32> {
        let i = tag as usize;
        match e.kind {
            Kind::Dir => {
                if e.mount_point {
                    return None;
                }
                self.note(|r| match (r.as_mut(), e.name.to_str()) {
                    (Some(rec), Some(name)) => rec.dirs.push((name.to_owned(), e.alloc)),
                    _ => *r = None, // a name the cache file cannot hold
                });
                if self.skipped(dir, &e.name) {
                    return None;
                }
                self.sizes[i].fetch_add(e.alloc, Ordering::Relaxed);
                self.left[i].fetch_add(1, Ordering::Relaxed);
                Some(tag)
            }
            _ => {
                self.files[i].fetch_add(1, Ordering::Relaxed);
                let single = e.nlink == 1;
                if single || self.links.first_time(e.dev, e.ino) {
                    self.sizes[i].fetch_add(e.alloc, Ordering::Relaxed);
                }
                self.note(|r| {
                    if let Some(rec) = r {
                        rec.files += 1;
                        if single {
                            rec.bytes += e.alloc;
                        } else {
                            rec.links.push((e.dev, e.ino, e.alloc));
                        }
                    }
                });
                None
            }
        }
    }

    fn error(&self, path: &Path, tag: u32, err: io::Error) {
        self.note(|r| *r = None);
        if err.kind() == io::ErrorKind::NotFound {
            return;
        }
        self.partial[tag as usize].store(true, Ordering::Relaxed);
        let mut errors = self.errors.lock().unwrap();
        if errors.len() < 20 {
            errors.push(format!("{}: {err}", path.display()));
        }
    }

    fn done(&self, _dir: &Path, tag: u32) {
        self.left[tag as usize].fetch_sub(1, Ordering::Relaxed);
        if let Some(rec) = self.cache.as_ref().and_then(|_| READING.take())
            // modified in the last seconds: it could change again within the same mtime
            && rec.mtime + 2 <= self.now
        {
            self.found.lock().unwrap().push(rec);
        }
    }
}

#[cfg(target_os = "macos")]
fn overview_roots() -> Vec<(String, PathBuf)> {
    let home = home();
    vec![
        ("Home".into(), home.clone()),
        ("User Library".into(), home.join("Library")),
        ("Applications".into(), PathBuf::from("/Applications")),
        ("System Library".into(), PathBuf::from("/Library")),
    ]
}

#[cfg(windows)]
fn overview_roots() -> Vec<(String, PathBuf)> {
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut roots = Vec::new();
    if let Some(p) = env("USERPROFILE") {
        roots.push(("Home".into(), p));
    }
    for (name, var) in [
        ("Programs", "ProgramFiles"),
        ("Programs (x86)", "ProgramFiles(x86)"),
        ("Program Data", "ProgramData"),
        ("Windows", "SystemRoot"),
    ] {
        if let Some(p) = env(var) {
            roots.push((name.into(), p));
        }
    }
    roots
}

#[cfg(not(any(target_os = "macos", windows)))]
fn overview_roots() -> Vec<(String, PathBuf)> {
    vec![("Home".into(), home())]
}

#[cfg(not(windows))]
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(windows)]
fn root_volume() -> String {
    std::env::var("SystemDrive")
        .map(|d| format!("{d}\\"))
        .unwrap_or_else(|_| "C:\\".into())
}

#[cfg(not(windows))]
fn root_volume() -> String {
    "/".into()
}

/// Bytes in use on the volume holding `path`.
#[cfg(target_os = "macos")]
fn volume_used(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    // the APFS container is shared by several volumes: f_blocks - f_bfree counts this one
    Some((st.f_blocks - st.f_bfree) * st.f_bsize as u64)
}

#[cfg(not(target_os = "macos"))]
fn volume_used(_path: &Path) -> Option<u64> {
    None
}
