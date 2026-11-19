//! Space Lens directory cache: what each directory's own entries added up to when it was
//! last read, keyed by (volume, file id) and checked against FSEvents or the directory's mtime.
//!
//! A re-scan takes a known directory from its record instead of listing it, and its
//! subdirectories (name, size, file id) come from the record too. Two ways to know that a
//! record still holds:
//!
//! 1. FSEvents (macOS). A scan notes the event id at its start in every record it writes,
//!    and each volume's event store UUID in a side file. The next scan replays the history
//!    of its roots since the oldest such id (`fsevents.rs`) and stats each directory the
//!    events name. FSEvents reports any change to a directory's entries, a file written in
//!    place included, as that directory; so a record no later event names is trusted
//!    *without a stat* (a repeat scan becomes table lookups) for `VOUCHED_AGE`, and one an
//!    event names is read again whatever its mtime.
//! 2. Otherwise (Windows; macOS when the history broke: dropped events, wrapped ids, a
//!    subtree to rescan, a mount, a volume whose event store changed, no stored id), the
//!    directory is stat-ed and its mtime compared. That covers entries added, removed or
//!    renamed, but NOT a file growing or shrinking in place (a log, a database, a VM disk):
//!    such a record is trusted for `MAX_AGE` only, and within that window a file that grew
//!    in place in an otherwise unchanged directory shows its old size.
//!
//! Limits of (1): events name paths, so a directory whose contents changed and which was
//! then renamed while another directory took its old path, all since the last scan, is
//! missed until `VOUCHED_AGE`. History the system purged early (it keeps days of it unless
//! the disk is nearly full) is not detected. The replay runs in the background after the
//! cache is read (tens to hundreds of ms); until it is in, records go by the mtime rule.
//!
//! That is acceptable for Space Lens, whose sizes are informational and which re-reads
//! everything at least every `VOUCHED_AGE`. It is not used for System Junk or `size`: those
//! sizes decide what is offered for removal (`clean.rs` drops items sized 0), and
//! "what deleting frees" (APFS private size) changes without any mtime changing.
//!
//! Memory: a whole startup disk is several hundred thousand directories, so the records
//! live in one sorted table (fixed-size rows, names and sizes in shared buffers, looked
//! up by binary search) and the file is read and written one line per directory.
//!
//! mtimes have one-second resolution here, so a directory modified in the second it was
//! read could change again unseen: such "hot" directories are not recorded.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

/// Seconds a record is trusted on its mtime alone (bounds how stale an in-place file
/// growth can be).
pub const MAX_AGE: i64 = 3600;
/// Seconds a record FSEvents vouches for is trusted; past that it is read again anyway.
pub const VOUCHED_AGE: i64 = 24 * 3600;
/// Seconds a record is kept in the file.
const KEPT: i64 = if cfg!(target_os = "macos") {
    VOUCHED_AGE
} else {
    MAX_AGE
};

/// One directory as read in a scan (and one line of the cache file).
#[derive(Clone, Debug, Default)]
pub struct Rec {
    pub dev: u64,
    pub ino: u64,
    pub mtime: i64,
    /// When it was read, seconds since the epoch
    pub at: i64,
    /// FSEvents id at the start of the scan that read it (0: none)
    pub ev: u64,
    /// Allocated bytes and number of its non-directory entries; `bytes` leaves out the
    /// hard-linked ones, which are in `links`
    pub bytes: u64,
    pub files: u64,
    /// Files whose link count is not 1 (or unknown): (dev, ino, allocated)
    pub links: Vec<(u64, u64, u64)>,
    /// Subdirectories, mount points excluded: their names joined by NUL (which no name
    /// contains), and their (allocated, file id) in the same order
    pub names: String,
    pub subs: Vec<(u64, u64)>,
}

impl Rec {
    pub fn add_dir(&mut self, name: &str, alloc: u64, ino: u64) {
        if !self.subs.is_empty() {
            self.names.push('\0');
        }
        self.names.push_str(name);
        self.subs.push((alloc, ino));
    }

    /// Gives back the spare capacity of a finished record.
    pub fn shrink(&mut self) {
        self.names.shrink_to_fit();
        self.subs.shrink_to_fit();
        self.links.shrink_to_fit();
    }
}

/// A row of the table: the record's numbers and where its parts are in the buffers.
#[derive(Clone, Copy)]
struct Row {
    dev: u64,
    ino: u64,
    mtime: i64,
    at: i64,
    ev: u64,
    bytes: u64,
    files: u64,
    names: (u32, u32),
    subs: (u32, u32),
    links: (u32, u32),
}

/// The records of a cache file, sorted by (dev, ino).
#[derive(Default)]
pub struct Table {
    /// records in the file, superseded and expired ones included
    read: usize,
    rows: Vec<Row>,
    names: String,
    subs: Vec<(u64, u64)>,
    links: Vec<(u64, u64, u64)>,
    /// Set once the FSEvents history is replayed (`None`: it cannot tell); until then
    /// the mtime rule decides
    pub history: OnceLock<Option<History>>,
}

/// What the FSEvents history says since the records were read.
#[derive(Default)]
pub struct History {
    /// Volumes whose history was replayed without a break: dev -> the first event id its
    /// current event store vouches from
    since: HashMap<u64, u64>,
    /// Directories named by an event in that history: (dev, ino) -> its last event id
    changed: HashMap<(u64, u64), u64>,
}

/// A record as the table holds it.
pub struct View<'a> {
    pub files: u64,
    pub bytes: u64,
    pub links: &'a [(u64, u64, u64)],
    mtime: i64,
    at: i64,
    ev: u64,
    names: &'a str,
    subs: &'a [(u64, u64)],
}

impl View<'_> {
    pub fn usable(&self, mtime: i64, now: i64) -> bool {
        self.mtime == mtime && now - self.at < MAX_AGE
    }

    /// (name, allocated, file id) of each subdirectory.
    pub fn dirs(&self) -> impl Iterator<Item = (&str, u64, u64)> {
        let names = (!self.subs.is_empty())
            .then(|| self.names.split('\0'))
            .into_iter()
            .flatten();
        names.zip(self.subs).map(|(n, &(a, i))| (n, a, i))
    }
}

impl Table {
    fn push(&mut self, r: &Rec) {
        let span = |start: usize, len: usize| (start as u32, len as u32);
        self.rows.push(Row {
            dev: r.dev,
            ino: r.ino,
            mtime: r.mtime,
            at: r.at,
            ev: r.ev,
            bytes: r.bytes,
            files: r.files,
            names: span(self.names.len(), r.names.len()),
            subs: span(self.subs.len(), r.subs.len()),
            links: span(self.links.len(), r.links.len()),
        });
        self.names.push_str(&r.names);
        self.subs.extend_from_slice(&r.subs);
        self.links.extend_from_slice(&r.links);
    }

    fn view(&self, row: &Row) -> View<'_> {
        let range = |(start, len): (u32, u32)| start as usize..(start + len) as usize;
        View {
            files: row.files,
            bytes: row.bytes,
            links: &self.links[range(row.links)],
            mtime: row.mtime,
            at: row.at,
            ev: row.ev,
            names: &self.names[range(row.names)],
            subs: &self.subs[range(row.subs)],
        }
    }

    fn rec(&self, row: &Row) -> Rec {
        let v = self.view(row);
        Rec {
            dev: row.dev,
            ino: row.ino,
            mtime: row.mtime,
            at: row.at,
            ev: row.ev,
            bytes: row.bytes,
            files: row.files,
            links: v.links.to_vec(),
            names: v.names.to_owned(),
            subs: v.subs.to_vec(),
        }
    }

    pub fn get(&self, key: (u64, u64)) -> Option<View<'_>> {
        let i = self
            .rows
            .binary_search_by_key(&key, |r| (r.dev, r.ino))
            .ok()?;
        Some(self.view(&self.rows[i]))
    }

    /// What FSEvents says about record `v` of directory `key` (whose `mtime` may be known):
    /// `Some(true)` nothing changed in it since it was read, `Some(false)` read it again
    /// (changed, or too old), `None` no usable history (the mtime rule decides).
    pub fn vouched(&self, key: (u64, u64), v: &View, mtime: Option<i64>, now: i64) -> Option<bool> {
        let h = self.history.get()?.as_ref()?;
        let since = *h.since.get(&key.0)?;
        if v.ev == 0 || v.ev < since {
            return None;
        }
        let changed =
            h.changed.get(&key).is_some_and(|&id| id > v.ev) || mtime.is_some_and(|m| m != v.mtime);
        Some(!changed && now - v.at < VOUCHED_AGE)
    }

    fn finish(mut self) -> Table {
        // rows were pushed in file order (appended records last); a stable sort keeps that
        // order among equal keys, and the last one of each key wins
        self.rows.reverse();
        self.rows.sort_by_key(|r| (r.dev, r.ino));
        self.rows.dedup_by_key(|r| (r.dev, r.ino));
        self.rows.shrink_to_fit();
        self.names.shrink_to_fit();
        self.subs.shrink_to_fit();
        self.links.shrink_to_fit();
        self
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// Cache folders a `store` of this process is writing to: `read` cuts a damaged tail only
/// when none is. ponytail: in-process only; another process appending at once is caught
/// by the size check alone.
static WRITING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Marks `dir` as being written until dropped.
struct Writing(PathBuf);

impl Writing {
    fn start(dir: &Path) -> Writing {
        writing().push(dir.to_path_buf());
        Writing(dir.to_path_buf())
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        let mut w = writing();
        if let Some(i) = w.iter().position(|d| *d == self.0) {
            w.swap_remove(i);
        }
    }
}

fn writing() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    WRITING.lock().unwrap_or_else(|p| p.into_inner())
}

fn file(dir: &Path) -> PathBuf {
    dir.join("space-dirs-v5.bin")
}

/// Per volume: dev, event store UUID, first event id records on it may rely on.
#[cfg(target_os = "macos")]
fn volumes_file(dir: &Path) -> PathBuf {
    dir.join("space-dirs-v5.vol")
}

#[cfg(target_os = "macos")]
fn read_volumes(dir: &Path) -> HashMap<u64, (u128, u64)> {
    let bytes = std::fs::read(volumes_file(dir)).unwrap_or_default();
    bytes
        .as_chunks::<32>()
        .0
        .iter()
        .map(|c| {
            let n = |i: usize| u64::from_le_bytes(c[i..i + 8].try_into().unwrap());
            let uuid = u128::from_le_bytes(c[8..24].try_into().unwrap());
            (n(0), (uuid, n(24)))
        })
        .collect()
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

// The file: records one after another in a small binary layout (a whole disk is tens of
// MB; JSON took seconds to read). New records are appended; a later record of the same
// directory wins; the file is rewritten when appends make up a large part of it.

fn put(out: &mut impl Write, r: &Rec) -> std::io::Result<()> {
    let nums = [
        r.dev,
        r.ino,
        r.mtime as u64,
        r.at as u64,
        r.ev,
        r.bytes,
        r.files,
    ];
    for n in nums {
        out.write_all(&n.to_le_bytes())?;
    }
    for n in [r.names.len(), r.subs.len(), r.links.len()] {
        out.write_all(&(n as u32).to_le_bytes())?;
    }
    out.write_all(r.names.as_bytes())?;
    for &(a, i) in &r.subs {
        for n in [a, i] {
            out.write_all(&n.to_le_bytes())?;
        }
    }
    for &(d, i, a) in &r.links {
        for n in [d, i, a] {
            out.write_all(&n.to_le_bytes())?;
        }
    }
    Ok(())
}

fn u64s(input: &mut impl Read, n: usize) -> std::io::Result<Vec<u64>> {
    let mut buf = vec![0u8; n * 8];
    input.read_exact(&mut buf)?;
    Ok(buf
        .as_chunks::<8>()
        .0
        .iter()
        .map(|c| u64::from_le_bytes(*c))
        .collect())
}

/// One record, or an error at the end of the file or on a damaged record: a record cut
/// short by a crash mid-append, or lengths that do not fit in the `left` bytes of the file
/// (never trusted for an allocation).
fn get(input: &mut impl Read, left: &mut u64) -> std::io::Result<Rec> {
    const HEAD: u64 = 7 * 8 + 3 * 4;
    let bad = || std::io::Error::from(std::io::ErrorKind::InvalidData);
    if *left < HEAD {
        return Err(bad());
    }
    let nums = u64s(input, 7)?;
    let mut lens = [0u8; 12];
    input.read_exact(&mut lens)?;
    let len = |i: usize| {
        u64::from(u32::from_le_bytes(
            lens[i * 4..i * 4 + 4].try_into().unwrap(),
        ))
    };
    let body = len(0) + 16 * len(1) + 24 * len(2);
    if body > *left - HEAD {
        return Err(bad());
    }
    let mut names = vec![0u8; len(0) as usize];
    input.read_exact(&mut names)?;
    let subs: Vec<(u64, u64)> = u64s(input, len(1) as usize * 2)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| (c[0], c[1]))
        .collect();
    let links = u64s(input, len(2) as usize * 3)?
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| (c[0], c[1], c[2]))
        .collect();
    let names = String::from_utf8(names).map_err(|_| bad())?;
    let count = if subs.is_empty() {
        0
    } else {
        names.split('\0').count()
    };
    // a subdirectory name is joined to its parent's path: one that is not a single plain
    // component (a forged or damaged file) could point the scan elsewhere
    let plain = |n: &str| {
        !(n.is_empty()
            || n == "."
            || n == ".."
            || n.contains('/')
            || cfg!(windows) && n.contains('\\'))
    };
    if count != subs.len() || !subs.is_empty() && !names.split('\0').all(plain) {
        return Err(bad());
    }
    *left -= HEAD + body;
    Ok(Rec {
        dev: nums[0],
        ino: nums[1],
        mtime: nums[2] as i64,
        at: nums[3] as i64,
        ev: nums[4],
        bytes: nums[5],
        files: nums[6],
        names,
        subs,
        links,
    })
}

use std::io::Read;

/// Reads the records kept in `dir` on another thread, then (macOS) replays the FSEvents
/// history of `roots`: a small folder is listed before a whole-disk cache could be read,
/// so the scan starts at once and uses the cache from the moment it is there, with the
/// mtime rule until the history is in.
pub fn load_in_background(dir: &Path, roots: Vec<PathBuf>) -> Arc<OnceLock<Arc<Table>>> {
    let slot = Arc::new(OnceLock::new());
    let (dir, fill) = (dir.to_path_buf(), slot.clone());
    std::thread::spawn(move || {
        let table = Arc::new(read(&dir));
        let _ = fill.set(table.clone());
        #[cfg(target_os = "macos")]
        let history = table.replay(&dir, &roots);
        #[cfg(not(target_os = "macos"))]
        let history = None;
        drop(roots);
        let _ = table.history.set(history);
    });
    slot
}

/// The FSEvents id a scan starting now writes in its records (0: no FSEvents).
pub fn event_id() -> u64 {
    #[cfg(target_os = "macos")]
    return crate::fsevents::current_id();
    #[cfg(not(target_os = "macos"))]
    0
}

#[cfg(target_os = "macos")]
impl Table {
    /// The history of `roots` since the oldest record that can rely on it; `None` when
    /// it broke or no record can.
    fn replay(&self, dir: &Path, roots: &[PathBuf]) -> Option<History> {
        let devs: Vec<u64> = roots
            .iter()
            .filter_map(|r| crate::fs::stat(r, crate::fs::Detail::Basic).ok())
            .map(|e| e.dev)
            .collect();
        let since: HashMap<u64, u64> = read_volumes(dir)
            .into_iter()
            .filter(|(dev, (uuid, _))| {
                devs.contains(dev) && crate::fsevents::volume_uuid(*dev) == Some(*uuid)
            })
            .map(|(dev, (_, since))| (dev, since))
            .collect();
        let from = self
            .rows
            .iter()
            .filter(|r| r.ev > 0 && since.get(&r.dev).is_some_and(|&s| r.ev >= s))
            .map(|r| r.ev)
            .min()?;
        let events = crate::fsevents::changes(roots, from, &dir.join("space-dirs-v5.sync"))?;
        let mut paths: HashMap<PathBuf, u64> = HashMap::new();
        for (path, id) in events {
            let last = paths.entry(path).or_default();
            *last = (*last).max(id);
        }
        let mut changed = HashMap::new();
        for (path, id) in paths {
            // gone: nothing to drop (its parent changed too)
            if let Ok(e) = crate::fs::stat(&path, crate::fs::Detail::Basic) {
                let last: &mut u64 = changed.entry((e.dev, e.ino)).or_default();
                *last = (*last).max(id);
            }
        }
        Some(History { since, changed })
    }
}

/// The records kept in `dir`.
pub fn read(dir: &Path) -> Table {
    let now = now();
    let mut table = Table::default();
    if let Ok(f) = std::fs::File::open(file(dir)) {
        let size = f.metadata().map_or(0, |m| m.len());
        let mut left = size;
        let mut input = std::io::BufReader::with_capacity(1 << 20, f);
        while left > 0 {
            let Ok(r) = get(&mut input, &mut left) else {
                // damaged tail (a crash mid-append): cut it off, so appends land after
                // the last good record instead of behind the damage. Not while a store of
                // this process writes, nor if the file grew since (an append may be what
                // looked damaged).
                let w = writing();
                if !w.iter().any(|d| d == dir)
                    && let Ok(f) = std::fs::OpenOptions::new().write(true).open(file(dir))
                    && f.metadata().is_ok_and(|m| m.len() == size)
                {
                    let _ = f.set_len(size - left); // under the lock: no store starts meanwhile
                }
                drop(w);
                break;
            };
            table.read += 1;
            if now - r.at < KEPT {
                table.push(&r);
            }
        }
    }
    table.finish()
}

/// Saves the records of a scan. `old` is the cache as read for that scan, if the read
/// finished (if not, nothing is saved). Few new records in a file that is not mostly
/// superseded records are appended; otherwise the file is rewritten with only what is
/// still fresh. Nothing is kept in memory afterwards.
pub fn store(dir: &Path, old: Option<Arc<Table>>, found: Vec<Rec>) {
    if found.is_empty() || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let _writing = Writing::start(dir);
    // (dev, the scan's event id) of what is saved
    #[cfg(target_os = "macos")]
    let found_devs: Vec<(u64, u64)> = {
        let mut v: Vec<(u64, u64)> = found.iter().map(|r| (r.dev, r.ev)).collect();
        v.sort_unstable();
        v.dedup_by_key(|d| d.0);
        v
    };
    let path = file(dir);
    let appending = match &old {
        // the background read has not finished (a quick scan): appending now could race
        // with it, and a quick scan's few folders are not worth it
        None if path.exists() => return,
        None => false,
        Some(t) => found.len() * 4 < t.rows.len() && t.read < t.rows.len() * 2,
    };
    let result = if appending {
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|f| {
                let mut out = std::io::BufWriter::new(f);
                found.iter().try_for_each(|r| put(&mut out, r))?;
                out.flush()
            })
    } else {
        let now = now();
        let mut found = found;
        found.sort_unstable_by_key(|r| (r.dev, r.ino));
        let tmp = crate::fs::tmp_path(dir, "space-dirs");
        let written = (|| -> std::io::Result<()> {
            let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
            for r in &found {
                put(&mut out, r)?;
            }
            if let Some(old) = &old {
                for row in &old.rows {
                    let replaced = found
                        .binary_search_by_key(&(row.dev, row.ino), |r| (r.dev, r.ino))
                        .is_ok();
                    if !replaced && now - row.at < KEPT {
                        put(&mut out, &old.rec(row))?;
                    }
                }
            }
            out.flush()
        })();
        written
            .and_then(|()| std::fs::rename(&tmp, &path))
            .inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
    };
    #[cfg(target_os = "macos")]
    if result.is_ok() {
        store_volumes(dir, &found_devs);
    }
    let _ = result;
    for older in [
        "space-dirs-v1.json",
        "space-dirs-v2.json",
        "space-dirs-v3.jsonl",
        "space-dirs-v4.bin",
    ] {
        let _ = std::fs::remove_file(dir.join(older));
    }
}

/// Notes each volume's event store UUID. A new one (or a first) vouches from the scan's
/// event id only: older records on it keep the mtime rule.
#[cfg(target_os = "macos")]
fn store_volumes(dir: &Path, found: &[(u64, u64)]) {
    let mut vols = read_volumes(dir);
    for &(dev, ev) in found {
        let Some(uuid) = crate::fsevents::volume_uuid(dev).filter(|_| ev > 0) else {
            vols.remove(&dev);
            continue;
        };
        if vols.get(&dev).is_none_or(|v| v.0 != uuid) {
            vols.insert(dev, (uuid, ev));
        }
    }
    let mut bytes = Vec::with_capacity(vols.len() * 32);
    for (dev, (uuid, since)) in vols {
        bytes.extend_from_slice(&dev.to_le_bytes());
        bytes.extend_from_slice(&uuid.to_le_bytes());
        bytes.extend_from_slice(&since.to_le_bytes());
    }
    let _ = crate::fs::write_atomic(&volumes_file(dir), bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ino: u64, bytes: u64) -> Rec {
        let mut r = Rec {
            dev: 1,
            ino,
            mtime: 5,
            at: now(),
            bytes,
            files: 1,
            ev: 42,
            ..Rec::default()
        };
        r.add_dir("a", 10, 1000);
        r.add_dir("b c", 20, 1001);
        r.links.push((1, 99, 7));
        r
    }

    #[test]
    fn appended_records_win_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("oc-dircache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        store(&dir, None, (1..=100).map(|i| rec(i, i)).collect());
        let t = Arc::new(read(&dir));
        store(&dir, Some(t), vec![rec(7, 7000)]); // few records: appended
        let t = read(&dir);
        assert_eq!(t.len(), 100);
        let v = t.get((1, 7)).unwrap();
        assert_eq!(v.bytes, 7000);
        assert_eq!(
            v.dirs().collect::<Vec<_>>(),
            vec![("a", 10, 1000), ("b c", 20, 1001)]
        );
        assert_eq!(v.links, &[(1, 99, 7)]);
        assert_eq!(v.ev, 42);
        assert_eq!(t.get((1, 8)).unwrap().bytes, 8);

        // a crash mid-append: a header promising more than the file holds, then garbage
        let path = file(&dir);
        let good = std::fs::metadata(&path).unwrap().len();
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        f.write_all(&[0xFF; 70]).unwrap();
        drop(f);
        let t = read(&dir);
        assert_eq!(t.len(), 100, "the good records survive");
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            good,
            "the damaged tail is cut off"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_with_a_path_for_a_name_is_damaged() {
        for bad in ["../up", "a/b", "..", "."] {
            let mut r = rec(1, 1);
            r.add_dir(bad, 1, 2);
            let mut bytes = Vec::new();
            put(&mut bytes, &r).unwrap();
            let mut left = bytes.len() as u64;
            assert!(get(&mut bytes.as_slice(), &mut left).is_err(), "{bad}");
        }
        let mut bytes = Vec::new();
        put(&mut bytes, &rec(1, 1)).unwrap();
        let mut left = bytes.len() as u64;
        assert!(get(&mut bytes.as_slice(), &mut left).is_ok());
    }

    /// Records read at event 42 on volume 1, then FSEvents history in various states.
    #[test]
    fn fsevents_vouch_or_fall_back() {
        let mut t = Table::default();
        for ino in [7, 8] {
            t.push(&rec(ino, 1));
        }
        let mut old = rec(9, 1);
        old.at -= VOUCHED_AGE;
        t.push(&old);
        let mut t = t.finish();
        let now = now();
        let ask = |t: &Table, ino| t.vouched((1, ino), &t.get((1, ino)).unwrap(), None, now);
        let with = |t: &mut Table, since: u64, changed: &[((u64, u64), u64)]| {
            t.history = OnceLock::new();
            let _ = t.history.set(Some(History {
                since: HashMap::from([(1, since)]),
                changed: changed.iter().copied().collect(),
            }));
        };

        // history not replayed yet, or broken (dropped, wrapped, rescan, missing id)
        assert_eq!(ask(&t, 7), None);
        let _ = t.history.set(None);
        assert_eq!(ask(&t, 7), None);
        // the volume's event store changed after the records were written: mtime rule
        with(&mut t, 50, &[]);
        assert_eq!(ask(&t, 7), None);

        // 8 changed after it was read, 7 before
        with(&mut t, 10, &[((1, 8), 43), ((1, 7), 40)]);
        assert_eq!(ask(&t, 7), Some(true), "no stat needed");
        assert_eq!(ask(&t, 8), Some(false), "read again whatever its mtime");
        assert_eq!(ask(&t, 9), Some(false), "past VOUCHED_AGE");
        let seven = t.get((1, 7)).unwrap();
        assert_eq!(
            t.vouched((1, 7), &seven, Some(6), now),
            Some(false),
            "listed mtime differs"
        );
        assert!(t.get((1, 7)).unwrap().usable(5, now));
    }
}
