//! Parallel tree walk. Workers share one LIFO queue of directories (depth-first keeps it
//! short on wide trees); each reads a whole directory in bulk, reports every entry to
//! the visitor and queues the subdirectories the visitor wants entered.

use super::{Entry, Kind};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

pub trait Visitor: Sync {
    /// Called for every entry of `dir`. For a directory, returning true walks into it.
    fn entry(&self, dir: &Path, entry: &Entry) -> bool;
    /// A directory (or an entry in it) could not be read.
    fn error(&self, path: &Path, err: io::Error);
}

/// Worker threads: I/O bound, so more than the core count keeps the disk queue full.
pub fn default_threads() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    (cores * 2).clamp(4, 64)
}

struct Queue {
    dirs: Vec<PathBuf>,
    /// directories queued or being read
    pending: usize,
}

/// Walks everything under `root` (a directory). Returns early, with a partial walk, once
/// `cancel` is set.
pub fn walk(root: &Path, threads: usize, cancel: &AtomicBool, visitor: &dyn Visitor) {
    let state = Mutex::new(Queue {
        dirs: vec![root.to_path_buf()],
        pending: 1,
    });
    let ready = Condvar::new();
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| worker(&state, &ready, cancel, visitor));
        }
    });
}

fn worker(state: &Mutex<Queue>, ready: &Condvar, cancel: &AtomicBool, visitor: &dyn Visitor) {
    let mut subdirs = Vec::new();
    loop {
        let dir = {
            let mut q = state.lock().unwrap();
            loop {
                if q.pending == 0 {
                    return;
                }
                if let Some(dir) = q.dirs.pop() {
                    break dir;
                }
                q = ready.wait(q).unwrap();
            }
        };

        subdirs.clear();
        if !cancel.load(Ordering::Relaxed) {
            let result = super::read_dir(&dir, &mut |e: &Entry| {
                if let Some(code) = e.error {
                    visitor.error(&dir.join(&e.name), io::Error::from_raw_os_error(code));
                    return;
                }
                if visitor.entry(&dir, e) && e.kind == Kind::Dir {
                    subdirs.push(dir.join(&e.name));
                }
            });
            if let Err(err) = result {
                visitor.error(&dir, err);
            }
        }

        let mut q = state.lock().unwrap();
        let added = subdirs.len();
        q.dirs.append(&mut subdirs);
        q.pending = q.pending + added - 1;
        drop(q);
        // wake idle workers for the new directories, and everyone when the walk is done
        ready.notify_all();
    }
}
