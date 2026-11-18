//! Tree sizes.
//!
//! - `Allocated`: what `du -skP` reports (with `one_volume`, `du -skPx`): blocks of every
//!   file and directory, a hard-linked file counted once, symlinks not followed.
//! - `Freed`: what deleting the tree gives back: regular files at their private size (APFS
//!   clone blocks shared with files elsewhere stay), and a hard-linked file only when all
//!   of its links are inside the tree. Never crosses into another volume.

use crate::fs::walk::{self, Visitor};
use crate::fs::{self, Detail, Entry, Kind};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Allocated,
    Freed,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub mode: Mode,
    /// Stay on the volume of the root (du -x). `Freed` always does.
    pub one_volume: bool,
    /// Report entries that vanish during the walk as failures (du does); otherwise they
    /// are ignored, as a file deleted mid-walk does not make the size wrong.
    pub missing_is_error: bool,
    pub threads: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            mode: Mode::Allocated,
            one_volume: true,
            missing_is_error: false,
            threads: walk::default_threads(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Report {
    pub bytes: u64,
    pub files: u64,
    pub dirs: u64,
    /// Entries that could not be read; `bytes` covers the rest.
    pub errors: u64,
    pub first_error: Option<String>,
    /// Every failure was a permission denial (the size is then a reliable lower bound).
    pub permission_only: bool,
    pub cancelled: bool,
}

impl Report {
    pub fn complete(&self) -> bool {
        self.errors == 0 && !self.cancelled
    }
}

/// Hard links seen so far, shared by the operands of one measurement (du counts a hard
/// link once per run, across all its operands).
#[derive(Default)]
pub struct Links(Mutex<HashSet<(u64, u64)>>);

impl Links {
    pub(crate) fn first_time(&self, dev: u64, ino: u64) -> bool {
        self.0.lock().unwrap().insert((dev, ino))
    }
}

pub fn measure(path: &Path, opts: &Options) -> io::Result<Report> {
    measure_with(path, opts, &Links::default(), &AtomicBool::new(false))
}

pub fn measure_with(
    path: &Path,
    opts: &Options,
    links: &Links,
    cancel: &AtomicBool,
) -> io::Result<Report> {
    let detail = if opts.mode == Mode::Freed {
        Detail::Freed
    } else {
        Detail::Basic
    };
    let root = fs::stat(path, detail)?;
    let mut report = Report::default();
    match (root.kind, opts.mode) {
        (Kind::Dir, _) => {}
        (Kind::Symlink, Mode::Freed) => return Ok(report),
        (Kind::File, Mode::Freed) => {
            report.files = 1;
            report.bytes = if root.nlink <= 1 { root.freed() } else { 0 };
            return Ok(report);
        }
        (_, Mode::Freed) => return Ok(report),
        (_, Mode::Allocated) => {
            report.files = 1;
            if root.nlink == 1 || links.first_time(root.dev, root.ino) {
                report.bytes = root.alloc;
            }
            return Ok(report);
        }
    }

    let sizer = Sizer {
        opts,
        links,
        bytes: AtomicU64::new(if opts.mode == Mode::Allocated {
            root.alloc
        } else {
            0
        }),
        files: AtomicU64::new(0),
        dirs: AtomicU64::new(1),
        errors: AtomicU64::new(0),
        non_permission: AtomicBool::new(false),
        first_error: Mutex::new(None),
        freed_links: Mutex::new(HashMap::new()),
    };
    walk::walk(path, opts.threads, detail, cancel, &sizer);

    let mut bytes = sizer.bytes.load(Ordering::Relaxed);
    for (_, (seen, nlink, size)) in sizer.freed_links.into_inner().unwrap() {
        if seen >= nlink {
            bytes += size;
        }
    }
    report.bytes = bytes;
    report.files = sizer.files.load(Ordering::Relaxed);
    report.dirs = sizer.dirs.load(Ordering::Relaxed);
    report.errors = sizer.errors.load(Ordering::Relaxed);
    report.permission_only = report.errors > 0 && !sizer.non_permission.load(Ordering::Relaxed);
    report.first_error = sizer.first_error.into_inner().unwrap();
    report.cancelled = cancel.load(Ordering::Relaxed);
    Ok(report)
}

/// Freed mode, a hard-linked file: (links seen in the tree, its link count, its size).
type FreedLink = (u32, u32, u64);

struct Sizer<'a> {
    opts: &'a Options,
    links: &'a Links,
    bytes: AtomicU64,
    files: AtomicU64,
    dirs: AtomicU64,
    errors: AtomicU64,
    non_permission: AtomicBool,
    first_error: Mutex<Option<String>>,
    freed_links: Mutex<HashMap<(u64, u64), FreedLink>>,
}

impl Visitor for Sizer<'_> {
    fn entry(&self, dir: &Path, tag: u32, e: &Entry) -> Option<u32> {
        match e.kind {
            Kind::Dir => {
                if e.mount_point {
                    if self.opts.one_volume || self.opts.mode == Mode::Freed {
                        return None;
                    }
                    // the bulk listing describes the directory under the mount: ask the mounted root
                    match fs::stat(&dir.join(&e.name), Detail::Basic) {
                        Ok(m) => self.add_alloc(m.alloc),
                        Err(err) => {
                            self.error(&dir.join(&e.name), tag, err);
                            return None;
                        }
                    }
                } else if self.opts.mode == Mode::Allocated {
                    self.add_alloc(e.alloc);
                }
                self.dirs.fetch_add(1, Ordering::Relaxed);
                Some(tag)
            }
            _ if self.opts.mode == Mode::Freed => {
                if e.kind == Kind::File {
                    self.files.fetch_add(1, Ordering::Relaxed);
                    let size = e.freed();
                    if e.nlink == 0 && !self.links.first_time(e.dev, e.ino) {
                        // unknown link count (Windows): once per file id
                        // ponytail: a hard link to a file outside the root counts as freed;
                        // query nNumberOfLinks per file if Windows rules ever meet such links
                    } else if e.nlink > 1 {
                        let mut map = self.freed_links.lock().unwrap();
                        let slot = map.entry((e.dev, e.ino)).or_insert((0, e.nlink, size));
                        slot.0 += 1;
                    } else {
                        self.add_alloc(size);
                    }
                }
                None
            }
            _ => {
                self.files.fetch_add(1, Ordering::Relaxed);
                if e.nlink == 1 || self.links.first_time(e.dev, e.ino) {
                    self.add_alloc(e.alloc);
                }
                None
            }
        }
    }

    fn error(&self, path: &Path, _tag: u32, err: io::Error) {
        if err.kind() == io::ErrorKind::NotFound && !self.opts.missing_is_error {
            return;
        }
        self.errors.fetch_add(1, Ordering::Relaxed);
        if err.kind() != io::ErrorKind::PermissionDenied {
            self.non_permission.store(true, Ordering::Relaxed);
        }
        let mut first = self.first_error.lock().unwrap();
        if first.is_none() {
            *first = Some(format!("{}: {}", path.display(), err));
        }
    }
}

impl Sizer<'_> {
    fn add_alloc(&self, n: u64) {
        self.bytes.fetch_add(n, Ordering::Relaxed);
    }
}

/// du-style kibibytes, rounded up.
pub fn kib(bytes: u64) -> u64 {
    bytes.div_ceil(1024)
}

/// Freed bytes of many items in one parallel walk (each walked directory is tagged with
/// its item), so a few big caches and many small ones spread evenly over the workers.
/// `progress` gets the running totals about every 200 ms. A hard-linked file counts for
/// an item only when all of its links are inside that item.
pub fn freed_many(
    items: &[std::path::PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&[u64]),
) -> Vec<u64> {
    many(items, Mode::Freed, cancel, progress)
}

/// Allocated bytes (du semantics) of many items in one parallel walk.
pub fn allocated_many(
    items: &[std::path::PathBuf],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&[u64]),
) -> Vec<u64> {
    many(items, Mode::Allocated, cancel, progress)
}

fn many(
    items: &[std::path::PathBuf],
    mode: Mode,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&[u64]),
) -> Vec<u64> {
    let detail = if mode == Mode::Freed {
        Detail::Freed
    } else {
        Detail::Basic
    };
    let tagged = Tagged {
        mode,
        bytes: items.iter().map(|_| AtomicU64::new(0)).collect(),
        links: Mutex::new(HashMap::new()),
        seen: Links::default(),
    };
    let mut roots = Vec::new();
    for (i, path) in items.iter().enumerate() {
        match fs::stat(path, detail) {
            Ok(e) if e.kind == Kind::Dir => {
                if mode == Mode::Allocated {
                    tagged.bytes[i].fetch_add(e.alloc, Ordering::Relaxed);
                }
                roots.push((path.clone(), i as u32));
            }
            Ok(e) => tagged.add(i, &e),
            Err(_) => {}
        }
    }
    std::thread::scope(|scope| {
        let walker = scope
            .spawn(|| walk::walk_tagged(roots, walk::default_threads(), detail, cancel, &tagged));
        let mut last = std::time::Instant::now();
        while !walker.is_finished() {
            std::thread::sleep(std::time::Duration::from_millis(20));
            if last.elapsed() >= std::time::Duration::from_millis(200) {
                progress(&tagged.snapshot());
                last = std::time::Instant::now();
            }
        }
    });
    let mut out = tagged.snapshot();
    for (_, (tag, seen, nlink, size)) in tagged.links.into_inner().unwrap() {
        if let Some(tag) = tag
            && seen >= nlink
        {
            out[tag] += size;
        }
    }
    out
}

/// (item the links were seen in, or None once seen in two items; links seen; link count; size)
type TaggedLink = (Option<usize>, u32, u32, u64);

struct Tagged {
    mode: Mode,
    bytes: Vec<AtomicU64>,
    /// Freed: hard links waiting to see whether all are inside one item
    links: Mutex<HashMap<(u64, u64), TaggedLink>>,
    /// Allocated: hard links already counted (once overall, like du)
    seen: Links,
}

impl Tagged {
    fn snapshot(&self) -> Vec<u64> {
        self.bytes
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .collect()
    }

    /// A non-directory entry of item `tag`.
    fn add(&self, tag: usize, e: &Entry) {
        if self.mode == Mode::Allocated {
            if e.nlink == 1 || self.seen.first_time(e.dev, e.ino) {
                self.bytes[tag].fetch_add(e.alloc, Ordering::Relaxed);
            }
            return;
        }
        if e.kind != Kind::File {
            return; // freed: regular files only
        }
        if e.nlink == 0 {
            // unknown link count (Windows): once per file id
            if self.seen.first_time(e.dev, e.ino) {
                self.bytes[tag].fetch_add(e.freed(), Ordering::Relaxed);
            }
            return;
        }
        if e.nlink == 1 {
            self.bytes[tag].fetch_add(e.freed(), Ordering::Relaxed);
            return;
        }
        let mut links = self.links.lock().unwrap();
        let slot = links
            .entry((e.dev, e.ino))
            .or_insert((Some(tag), 0, e.nlink, e.freed()));
        if slot.0 != Some(tag) {
            slot.0 = None;
        }
        slot.1 += 1;
    }
}

impl Visitor for Tagged {
    fn entry(&self, _dir: &Path, tag: u32, e: &Entry) -> Option<u32> {
        match e.kind {
            Kind::Dir if e.mount_point => None,
            Kind::Dir => {
                if self.mode == Mode::Allocated {
                    self.bytes[tag as usize].fetch_add(e.alloc, Ordering::Relaxed);
                }
                Some(tag)
            }
            _ => {
                self.add(tag as usize, e);
                None
            }
        }
    }

    fn error(&self, _path: &Path, _tag: u32, _err: io::Error) {}
}
