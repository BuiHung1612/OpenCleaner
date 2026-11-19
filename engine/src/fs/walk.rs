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
    /// Before `dir` is read: a visitor that already knows it (from a cache) accounts for
    /// it here, adds the subdirectories to walk to `subdirs` and returns true; `dir` is
    /// then not read.
    fn known(&self, _dir: &Path, _tag: u32, _subdirs: &mut Vec<(PathBuf, u32)>) -> bool {
        false
    }
    /// `dir` is finished: read (or failed to read), or known. Not called once cancelled.
    fn done(&self, _dir: &Path, _tag: u32) {}
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
        if !cancel.load(Ordering::Relaxed) && !visitor.known(&dir, tag, &mut subdirs) {
            let result = super::read_dir_cancellable(&dir, detail, cancel, &mut |e: &Entry| {
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
            // a listing stopped by cancel is not an error the user should see
            if let Err(err) = result
                && !cancel.load(Ordering::Relaxed)
            {
                visitor.error(&dir, tag, err);
            }
        }
        if !cancel.load(Ordering::Relaxed) {
            visitor.done(&dir, tag);
        }

        let mut q = state.lock().unwrap();
        let added = subdirs.len();
        q.dirs.append(&mut subdirs);
        q.pending = q.pending + added - 1;
        let finished = q.pending == 0;
        drop(q);
        // Workers sleep only while the queue is empty and someone is still reading, so they
        // need waking for exactly two things:
        // - the walk is over (pending == 0): wake everyone so they return. Once cancelled,
        //   nobody queues anything, so pending reaches 0 as the queued directories are
        //   skipped and this wakes everyone; waking all on cancel too just drains faster.
        // - new directories: this worker takes one of them itself on its next turn, so wake
        //   one sleeper per other directory. A wake-up that finds the queue already emptied
        //   (by a worker that was not asleep) goes back to sleep; a notify with no sleeper
        //   is harmless, as every awake worker pops before it waits.
        // notify_all after every directory woke every idle worker to fight over one entry.
        if finished || cancel.load(Ordering::Relaxed) {
            ready.notify_all();
        } else {
            for _ in 1..added {
                ready.notify_one();
            }
        }
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
