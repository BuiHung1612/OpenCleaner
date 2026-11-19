//! The user's own files worth a look: big ones nobody opened for months, old screenshots,
//! and exact duplicates. Everything goes to the Trash, never deleted outright.

use crate::context::Context;
use crate::fs::walk::{self, Visitor};
use crate::fs::{self, Detail, Entry, Kind};
use crate::guard::{self, Purpose};
use crate::picks::{Action, Pick};
use std::collections::{HashMap, HashSet};
use std::hash::Hasher;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

const DAY: i64 = 86_400;
const MIB: u64 = 1 << 20;

/// Where people keep their own files.
const FOLDERS: &[&str] = &[
    "Downloads",
    "Documents",
    "Desktop",
    "Pictures",
    "Movies",
    "Music",
    "Videos",
];

/// Folders that are one thing to the user (an app, a Photos or Music library, a project
/// bundle): never looked into, so nothing inside them is offered on its own.
const PACKAGES: &[&str] = &[
    "app",
    "photoslibrary",
    "musiclibrary",
    "tvlibrary",
    "imovielibrary",
    "fcpbundle",
    "logicx",
    "band",
    "bundle",
    "framework",
    "plugin",
    "photolibrary",
    "aplibrary",
    "lrdata",
    "lrcat-data",
    "pages",
    "numbers",
    "key",
    "rtfd",
    "xcodeproj",
    "xcworkspace",
];

fn is_package(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| PACKAGES.contains(&e.to_string_lossy().to_ascii_lowercase().as_str()))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// A file found in the user's folders: (path, root it was found under, entry).
type Found = (PathBuf, PathBuf, Entry);

struct Collect<'a> {
    roots: &'a [PathBuf],
    keep: &'a (dyn Fn(&Path, &Entry) -> bool + Sync),
    /// folders the user excluded in Settings: not looked into
    excluded: Vec<PathBuf>,
    found: Mutex<Vec<Found>>,
}

impl Visitor for Collect<'_> {
    fn entry(&self, dir: &Path, tag: u32, e: &Entry) -> Option<u32> {
        let name = e.name.to_string_lossy();
        match e.kind {
            Kind::Dir
                if !e.mount_point
                    && !name.starts_with('.')
                    && !is_package(&name)
                    && !self.excluded.contains(&dir.join(&e.name)) =>
            {
                Some(tag)
            }
            Kind::File if (self.keep)(dir, e) => {
                self.found.lock().unwrap().push((
                    dir.join(&e.name),
                    self.roots[tag as usize].clone(),
                    e.clone(),
                ));
                None
            }
            _ => None,
        }
    }

    fn error(&self, _path: &Path, _tag: u32, _err: io::Error) {}
}

/// Walks the user's folders (each once) and keeps the files `keep` accepts.
fn collect(
    ctx: &Context,
    keep: &(dyn Fn(&Path, &Entry) -> bool + Sync),
    cancel: &AtomicBool,
) -> Vec<Found> {
    let mut seen = HashSet::new();
    let roots: Vec<PathBuf> = FOLDERS
        .iter()
        .map(|f| ctx.home.join(f))
        .filter(|r| {
            !guard::excluded(r, ctx)
                && fs::stat(r, Detail::Basic)
                    .is_ok_and(|e| e.kind == Kind::Dir && seen.insert((e.dev, e.ino)))
        })
        .collect();
    let visitor = Collect {
        roots: &roots,
        keep,
        excluded: guard::exclusions(ctx),
        found: Mutex::new(Vec::new()),
    };
    walk::walk_tagged(
        roots
            .iter()
            .enumerate()
            .map(|(i, r)| (r.clone(), i as u32))
            .collect(),
        walk::default_threads(),
        Detail::Freed,
        cancel,
        &visitor,
    );
    visitor.found.into_inner().unwrap()
}

fn pick(path: PathBuf, root: PathBuf, e: &Entry, ctx: &Context) -> Pick {
    let mut p = Pick::new(path, root, Purpose::Files, Action::Trash);
    p.bytes = e.freed();
    p.group = p
        .path
        .parent()
        .and_then(|d| d.strip_prefix(&ctx.home).ok())
        .map(|d| format!("~/{}", d.display()))
        .unwrap_or_default();
    p
}

fn is_screenshot(name: &str) -> bool {
    let lower = name.to_lowercase();
    [
        "screenshot",
        "screen shot",
        "screen recording",
        "ảnh màn hình",
        "bản ghi màn hình",
    ]
    .iter()
    .any(|s| lower.starts_with(s))
}

/// Files of at least `min_mib` MiB not opened or changed for `days` days, and screenshots
/// or screen recordings on the Desktop older than 30 days (those start selected).
pub fn large_old(ctx: &Context, min_mib: u64, days: i64, cancel: &AtomicBool) -> Vec<Pick> {
    let now = now();
    let desktop = ctx.home.join("Desktop");
    let keep = |dir: &Path, e: &Entry| {
        let idle = now - e.mtime.max(e.atime);
        (e.len >= min_mib * MIB && idle >= days * DAY)
            || (dir == desktop
                && is_screenshot(&e.name.to_string_lossy())
                && now - e.mtime >= 30 * DAY)
    };
    let mut picks: Vec<Pick> = collect(ctx, &keep, cancel)
        .into_iter()
        .map(|(path, root, e)| {
            let shot = path.parent() == Some(desktop.as_path())
                && is_screenshot(&e.name.to_string_lossy());
            let idle_days = (now - e.mtime.max(e.atime)) / DAY;
            let mut p = pick(path, root, &e, ctx);
            if shot {
                p.kind = "screenshot".into();
                p.age_days = Some((now - e.mtime) / DAY);
                p.detail = format!("Screenshot · {} days old", (now - e.mtime) / DAY);
                p.preselect = true;
            } else {
                p.idle_days = Some(idle_days);
                p.detail = format!("Not opened for {idle_days} days");
            }
            p
        })
        .collect();
    picks.sort_by_key(|p| std::cmp::Reverse(p.bytes));
    picks
}

/// Files of at least `min_mib` MiB whose contents are byte-for-byte the same. Each set
/// keeps one copy unticked (the oldest, likely the original); the rest start selected.
pub fn duplicates(ctx: &Context, min_mib: u64, cancel: &AtomicBool) -> Vec<Pick> {
    let min = min_mib.max(1) * MIB;
    let keep = |_: &Path, e: &Entry| e.len >= min;
    // same size first (free), then the same first 64 KiB, then the whole content
    let mut by_len: HashMap<u64, Vec<Found>> = HashMap::new();
    let mut inodes = HashSet::new();
    for f in collect(ctx, &keep, cancel) {
        // a hard link is the same file, not a copy
        if inodes.insert((f.2.dev, f.2.ino)) {
            by_len.entry(f.2.len).or_default().push(f);
        }
    }
    let mut picks = Vec::new();
    for (_, same_len) in by_len.into_iter().filter(|(_, v)| v.len() > 1) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        for same_head in split_by(same_len, |p| hash(p, Some(64 * 1024))) {
            for same in split_by(same_head, |p| hash(p, None)) {
                for set in confirm(same) {
                    picks.extend(set_picks(set, ctx));
                }
            }
        }
    }
    picks.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.group.cmp(&b.group)));
    picks
}

/// Groups of two or more with the same key; unreadable files drop out.
fn split_by(files: Vec<Found>, key: impl Fn(&Path) -> Option<u64>) -> Vec<Vec<Found>> {
    let mut groups: HashMap<u64, Vec<Found>> = HashMap::new();
    for f in files {
        if let Some(k) = key(&f.0) {
            groups.entry(k).or_default().push(f);
        }
    }
    groups.into_values().filter(|g| g.len() > 1).collect()
}

fn hash(path: &Path, limit: Option<u64>) -> Option<u64> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader: Box<dyn Read> = match limit {
        Some(n) => Box::new(file.take(n)),
        None => Box::new(file),
    };
    let mut h = std::hash::DefaultHasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 {
            return Some(h.finish());
        }
        h.write(&buf[..n]);
    }
}

/// A hash match is not proof: compare the bytes against the first file of the set.
fn confirm(mut same: Vec<Found>) -> Vec<Vec<Found>> {
    let first = same.remove(0);
    let (equal, other): (Vec<Found>, Vec<Found>) =
        same.into_iter().partition(|f| same_bytes(&first.0, &f.0));
    let mut out = Vec::new();
    if !equal.is_empty() {
        let mut set = vec![first];
        set.extend(equal);
        out.push(set);
    }
    if other.len() > 1 {
        out.extend(confirm(other));
    }
    out
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let (Ok(na), Ok(nb)) = (read_full(&mut fa, &mut ba), read_full(&mut fb, &mut bb)) else {
            return false;
        };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

fn set_picks(mut set: Vec<Found>, ctx: &Context) -> Vec<Pick> {
    // the oldest copy stays
    set.sort_by_key(|f| f.2.mtime);
    let name = set[0].2.name.to_string_lossy().into_owned();
    let count = set.len();
    set.into_iter()
        .enumerate()
        .map(|(i, (path, root, e))| {
            let mut p = pick(path, root, &e, ctx);
            let place = p.group.clone();
            p.group = format!("{name} · {count} copies");
            p.copies = Some(count);
            p.place = place.clone();
            if i == 0 {
                p.flags.push("kept");
            }
            p.detail = if i == 0 {
                format!("{place} · oldest, kept")
            } else {
                place
            };
            p.preselect = i > 0;
            p
        })
        .collect()
}
