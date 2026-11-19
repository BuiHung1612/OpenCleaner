//! Parallel tree walk. Workers share one LIFO queue of directories (depth-first keeps it
//! short on wide trees); each reads a whole directory in bulk, reports every entry to
//! the visitor and queues the subdirectories the visitor wants entered.
//!
//! Every queued directory carries a tag, inherited by what is found below it unless the
//! visitor gives a new one. One walk can so size many roots, or every child of a
//! folder, at once: the work spreads over all workers however uneven the parts are.

use super::{Detail, Entry, Kind};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};

pub trait Visitor: Sync {
    /// Called for every entry of `dir` (which carries `tag`). For a directory, returning
    /// a tag walks into it with that tag; `None` skips it.
    fn entry(&self, dir: &Path, tag: u32, entry: &Entry) -> Option<u32>;
    /// A directory (or an entry in it, under the directory tagged `tag`) could not be read.
    fn error(&self, path: &Path, tag: u32, err: io::Error);
}

/// Fast mode: scans take every core at normal priority. Off (the default), they leave a
/// quarter of the cores free and run below interactive work, so the machine stays smooth.
pub static FAST: AtomicBool = AtomicBool::new(false);

/// Worker threads: one per core when fast, three per four cores otherwise. More threads
/// were no faster (the time goes to the kernel reading directories).
pub fn default_threads() -> usize {
    if let Some(n) = std::env::var("OC_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        return n.clamp(1, 256);
    }
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    if FAST.load(Ordering::Relaxed) {
        cores.clamp(4, 64)
    } else {
        (cores * 3 / 4).clamp(2, 48)
    }
}

struct Queue {
    dirs: Vec<(PathBuf, u32)>,
    /// directories queued or being read
    pending: usize,
}

/// Walks everything under `root` (a directory), tagged 0. Returns early, with a partial
/// walk, once `cancel` is set.
pub fn walk(
    root: &Path,
    threads: usize,
    detail: Detail,
    cancel: &AtomicBool,
    visitor: &dyn Visitor,
) {
    walk_tagged(
        vec![(root.to_path_buf(), 0)],
        threads,
        detail,
        cancel,
        visitor,
    );
}

/// Walks several directories in one pool, each with its tag.
pub fn walk_tagged(
    roots: Vec<(PathBuf, u32)>,
    threads: usize,
    detail: Detail,
    cancel: &AtomicBool,
    visitor: &dyn Visitor,
) {
    if roots.is_empty() {
        return;
    }
    let pending = roots.len();
    let state = Mutex::new(Queue {
        dirs: roots,
        pending,
    });
    let ready = Condvar::new();
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| worker(&state, &ready, detail, cancel, visitor));
        }
    });
}

fn worker(
    state: &Mutex<Queue>,
    ready: &Condvar,
    detail: Detail,
    cancel: &AtomicBool,
    visitor: &dyn Visitor,
) {
    background();
    let mut subdirs = Vec::new();
    loop {
        let (dir, tag) = {
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
            let result = super::read_dir(&dir, detail, &mut |e: &Entry| {
                if let Some(code) = e.error {
                    visitor.error(&dir.join(&e.name), tag, io::Error::from_raw_os_error(code));
                    return;
                }
                if let Some(sub_tag) = visitor.entry(&dir, tag, e)
                    && e.kind == Kind::Dir
                {
                    subdirs.push((dir.join(&e.name), sub_tag));
                }
            });
            if let Err(err) = result {
                visitor.error(&dir, tag, err);
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

/// Runs this thread below interactive work: the machine stays responsive during a scan
/// or a removal, and the disk serves other apps first when they need it.
pub fn background() {
    if FAST.load(Ordering::Relaxed) {
        return;
    }
    #[cfg(target_os = "macos")]
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
    }
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}
