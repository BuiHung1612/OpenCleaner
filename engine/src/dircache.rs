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
//! mtimes have one-second resolution here, so a directory modified in the second it was
//! read could change again unseen: such "hot" directories are not recorded.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Seconds a record is trusted (bounds how stale an in-place file growth can be).
pub const MAX_AGE: i64 = 3600;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
    /// Subdirectories, mount points excluded: (name, allocated)
    pub dirs: Vec<(String, u64)>,
}

impl Rec {
    pub fn usable(&self, mtime: i64, now: i64) -> bool {
        self.mtime == mtime && now - self.at < MAX_AGE
    }
}

pub type Map = HashMap<(u64, u64), Rec>;

static CACHE: Mutex<Option<(PathBuf, Arc<Map>)>> = Mutex::new(None);

fn file(dir: &Path) -> PathBuf {
    dir.join("space-dirs-v1.json")
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The records kept in `dir` (loaded once per process).
pub fn load(dir: &Path) -> Arc<Map> {
    let mut cache = CACHE.lock().unwrap();
    if let Some((d, map)) = &*cache
        && d == dir
    {
        return map.clone();
    }
    let now = now();
    let recs: Vec<Rec> = std::fs::read(file(dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let map: Arc<Map> = Arc::new(
        recs.into_iter()
            .filter(|r| now - r.at < MAX_AGE)
            .map(|r| ((r.dev, r.ino), r))
            .collect(),
    );
    *cache = Some((dir.to_path_buf(), map.clone()));
    map
}

/// Adds the records of a scan, drops expired ones and writes the cache to `dir`.
pub fn store(dir: &Path, found: Vec<Rec>) {
    if found.is_empty() {
        return;
    }
    let now = now();
    let map = {
        let mut cache = CACHE.lock().unwrap();
        if !matches!(&*cache, Some((d, _)) if d == dir) {
            *cache = Some((dir.to_path_buf(), Arc::default()));
        }
        let (_, slot) = cache.as_mut().unwrap();
        let map_mut = Arc::make_mut(slot);
        map_mut.retain(|_, r| now - r.at < MAX_AGE);
        for r in found {
            map_mut.insert((r.dev, r.ino), r);
        }
        slot.clone()
    };
    let recs: Vec<&Rec> = map.values().collect();
    let Ok(bytes) = serde_json::to_vec(&recs) else {
        return;
    };
    let tmp = dir.join(format!(".space-dirs-{}.tmp", std::process::id()));
    if std::fs::create_dir_all(dir).is_ok() && std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, file(dir));
    }
}
