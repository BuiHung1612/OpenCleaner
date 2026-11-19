//! Space Lens directory cache: what each directory's own entries added up to when it was
//! last read, keyed by (volume, file id) and checked against the directory's mtime.
//!
//! A re-scan stats a known directory instead of listing it: one call instead of one
//! bulk listing of all its files, and the subdirectories come from the record.
//!
//! What a directory's mtime does and does not cover:
//! - entries added, removed or renamed in it change it, so those are always seen;
//! - a file growing or shrinking in place (a log, a database, a VM disk) does NOT. A
//!   record is therefore trusted for `MAX_AGE` only: within that window a file that grew
//!   in place in an otherwise unchanged directory shows its old size.
//!
//! That is acceptable for Space Lens, whose sizes are informational and which re-reads
//! everything at least every `MAX_AGE`. It is not used for System Junk or `size`: those
//! sizes decide what is offered for removal (`clean.rs` drops items sized 0), and
//! "what deleting frees" (APFS private size) changes without any mtime changing.
//!
//! Memory: a whole startup disk is several hundred thousand directories, so the records
//! live in one sorted table (fixed-size rows, names and sizes in shared buffers, looked
//! up by binary search) and the file is read and written one line per directory.
//!
//! mtimes have one-second resolution here, so a directory modified in the second it was
//! read could change again unseen: such "hot" directories are not recorded.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Seconds a record is trusted (bounds how stale an in-place file growth can be).
pub const MAX_AGE: i64 = 3600;

/// One directory as read in a scan (and one line of the cache file).
#[derive(Clone, Debug, Default)]
pub struct Rec {
    pub dev: u64,
    pub ino: u64,
    pub mtime: i64,
    /// When it was read, seconds since the epoch
    pub at: i64,
    /// Allocated bytes and number of its non-directory entries; `bytes` leaves out the
    /// hard-linked ones, which are in `links`
    pub bytes: u64,
    pub files: u64,
    /// Files whose link count is not 1 (or unknown): (dev, ino, allocated)
    pub links: Vec<(u64, u64, u64)>,
    /// Subdirectories, mount points excluded: their names joined by NUL (which no name
    /// contains) and their allocated sizes in the same order
    pub names: String,
    pub allocs: Vec<u64>,
}

impl Rec {
    pub fn add_dir(&mut self, name: &str, alloc: u64) {
        if !self.allocs.is_empty() {
            self.names.push('\0');
        }
        self.names.push_str(name);
        self.allocs.push(alloc);
    }

    /// Gives back the spare capacity of a finished record.
    pub fn shrink(&mut self) {
        self.names.shrink_to_fit();
        self.allocs.shrink_to_fit();
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
    bytes: u64,
    files: u64,
    names: (u32, u32),
    allocs: (u32, u32),
    links: (u32, u32),
}

/// The records of a cache file, sorted by (dev, ino).
#[derive(Default)]
pub struct Table {
    rows: Vec<Row>,
    names: String,
    allocs: Vec<u64>,
    links: Vec<(u64, u64, u64)>,
}

/// A record as the table holds it.
pub struct View<'a> {
    pub files: u64,
    pub bytes: u64,
    pub links: &'a [(u64, u64, u64)],
    mtime: i64,
    at: i64,
    names: &'a str,
    allocs: &'a [u64],
}

impl View<'_> {
    pub fn usable(&self, mtime: i64, now: i64) -> bool {
        self.mtime == mtime && now - self.at < MAX_AGE
    }

    /// (name, allocated) of each subdirectory.
    pub fn dirs(&self) -> impl Iterator<Item = (&str, u64)> {
        let names = (!self.allocs.is_empty())
            .then(|| self.names.split('\0'))
            .into_iter()
            .flatten();
        names.zip(self.allocs.iter().copied())
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
            bytes: r.bytes,
            files: r.files,
            names: span(self.names.len(), r.names.len()),
            allocs: span(self.allocs.len(), r.allocs.len()),
            links: span(self.links.len(), r.links.len()),
        });
        self.names.push_str(&r.names);
        self.allocs.extend_from_slice(&r.allocs);
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
            names: &self.names[range(row.names)],
            allocs: &self.allocs[range(row.allocs)],
        }
    }

    fn rec(&self, row: &Row) -> Rec {
        let v = self.view(row);
        Rec {
            dev: row.dev,
            ino: row.ino,
            mtime: row.mtime,
            at: row.at,
            bytes: row.bytes,
            files: row.files,
            links: v.links.to_vec(),
            names: v.names.to_owned(),
            allocs: v.allocs.to_vec(),
        }
    }

    pub fn get(&self, key: (u64, u64)) -> Option<View<'_>> {
        let i = self
            .rows
            .binary_search_by_key(&key, |r| (r.dev, r.ino))
            .ok()?;
        Some(self.view(&self.rows[i]))
    }

    fn finish(mut self) -> Table {
        // rows were pushed in file order (appended records last); a stable sort keeps that
        // order among equal keys, and the last one of each key wins
        self.rows.reverse();
        self.rows.sort_by_key(|r| (r.dev, r.ino));
        self.rows.dedup_by_key(|r| (r.dev, r.ino));
        self.rows.shrink_to_fit();
        self.names.shrink_to_fit();
        self.allocs.shrink_to_fit();
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

static CACHE: Mutex<Option<(PathBuf, Arc<Table>)>> = Mutex::new(None);

fn file(dir: &Path) -> PathBuf {
    dir.join("space-dirs-v4.bin")
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
    let nums = [r.dev, r.ino, r.mtime as u64, r.at as u64, r.bytes, r.files];
    for n in nums {
        out.write_all(&n.to_le_bytes())?;
    }
    for n in [r.names.len(), r.allocs.len(), r.links.len()] {
        out.write_all(&(n as u32).to_le_bytes())?;
    }
    out.write_all(r.names.as_bytes())?;
    for a in &r.allocs {
        out.write_all(&a.to_le_bytes())?;
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

fn get(input: &mut impl Read) -> std::io::Result<Rec> {
    let nums = u64s(input, 6)?;
    let mut lens = [0u8; 12];
    input.read_exact(&mut lens)?;
    let len = |i: usize| u32::from_le_bytes(lens[i * 4..i * 4 + 4].try_into().unwrap()) as usize;
    let mut names = vec![0u8; len(0)];
    input.read_exact(&mut names)?;
    let allocs = u64s(input, len(1))?;
    let links = u64s(input, len(2) * 3)?
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| (c[0], c[1], c[2]))
        .collect();
    Ok(Rec {
        dev: nums[0],
        ino: nums[1],
        mtime: nums[2] as i64,
        at: nums[3] as i64,
        bytes: nums[4],
        files: nums[5],
        names: String::from_utf8(names).map_err(std::io::Error::other)?,
        allocs,
        links,
    })
}

use std::io::Read;

/// The records kept in `dir` (kept for the scan that asked, then freed by `store`).
pub fn load(dir: &Path) -> Arc<Table> {
    let mut cache = CACHE.lock().unwrap();
    if let Some((d, table)) = &*cache
        && d == dir
    {
        return table.clone();
    }
    let now = now();
    let mut table = Table::default();
    if let Ok(f) = std::fs::File::open(file(dir)) {
        let mut input = std::io::BufReader::with_capacity(1 << 20, f);
        while let Ok(r) = get(&mut input) {
            if now - r.at < MAX_AGE {
                table.push(&r);
            }
        }
    }
    let table = Arc::new(table.finish());
    *cache = Some((dir.to_path_buf(), table.clone()));
    table
}

/// Saves the records of a scan, then lets the table go: the app would otherwise hold it
/// for its lifetime. Small scans append; a large share of new records rewrites the file
/// with only what is still fresh.
pub fn store(dir: &Path, found: Vec<Rec>) {
    let old = CACHE
        .lock()
        .unwrap()
        .take()
        .filter(|(d, _)| d == dir)
        .map(|(_, t)| t);
    if found.is_empty() || std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let kept = old.as_ref().map_or(0, |t| t.rows.len());
    let path = file(dir);
    let on_disk = std::fs::metadata(&path).map_or(0, |m| m.len());
    let appending = found.len() * 4 < kept && on_disk < 256 << 20;
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
        let tmp = dir.join(format!(".space-dirs-{}.tmp", std::process::id()));
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
                    if !replaced && now - row.at < MAX_AGE {
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
    let _ = result;
    for older in [
        "space-dirs-v1.json",
        "space-dirs-v2.json",
        "space-dirs-v3.jsonl",
    ] {
        let _ = std::fs::remove_file(dir.join(older));
    }
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
            ..Rec::default()
        };
        r.add_dir("a", 10);
        r.add_dir("b c", 20);
        r.links.push((1, 99, 7));
        r
    }

    #[test]
    fn appended_records_win_and_round_trip() {
        let dir = std::env::temp_dir().join(format!("oc-dircache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        store(&dir, (1..=100).map(|i| rec(i, i)).collect());
        load(&dir);
        store(&dir, vec![rec(7, 7000)]); // few records: appended
        let t = load(&dir);
        assert_eq!(t.len(), 100);
        let v = t.get((1, 7)).unwrap();
        assert_eq!(v.bytes, 7000);
        assert_eq!(v.dirs().collect::<Vec<_>>(), vec![("a", 10), ("b c", 20)]);
        assert_eq!(v.links, &[(1, 99, 7)]);
        assert_eq!(t.get((1, 8)).unwrap().bytes, 8);
        store(&dir, Vec::new());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
