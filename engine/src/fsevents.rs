//! macOS FSEvents history: which directories changed since an event id, so the Space Lens
//! cache (`dircache`) can trust a record without even stat-ing its directory.
//!
//! FSEvents reports at directory granularity: a file created, removed, renamed or written
//! to (also in place) shows up as its parent directory. The history is replayed once per
//! scan with a short-lived stream; anything that makes it incomplete (dropped events,
//! wrapped ids, a subtree to rescan, a mount) makes `changes` return `None` and the cache
//! falls back to its mtime rule.

use std::ffi::{CStr, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

const MUST_SCAN_SUB_DIRS: u32 = 0x01;
const USER_DROPPED: u32 = 0x02;
const KERNEL_DROPPED: u32 = 0x04;
const EVENT_IDS_WRAPPED: u32 = 0x08;
const HISTORY_DONE: u32 = 0x10;
const ROOT_CHANGED: u32 = 0x20;
const MOUNT: u32 = 0x40;
const UNMOUNT: u32 = 0x80;
const CREATE_NO_DEFER: u32 = 0x02;

/// An event after which the history no longer says, directory by directory, what changed.
pub fn breaks_history(flags: u32) -> bool {
    flags
        & (MUST_SCAN_SUB_DIRS
            | USER_DROPPED
            | KERNEL_DROPPED
            | EVENT_IDS_WRAPPED
            | ROOT_CHANGED
            | MOUNT
            | UNMOUNT)
        != 0
}

type Stream = *mut c_void;
type Callback = extern "C" fn(Stream, *mut c_void, usize, *mut c_void, *const u32, *const u64);

#[repr(C)]
struct Context {
    version: isize,
    info: *mut c_void,
    retain: *const c_void,
    release: *const c_void,
    copy_description: *const c_void,
}

#[repr(C)]
struct UuidBytes([u8; 16]);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn FSEventsGetCurrentEventId() -> u64;
    fn FSEventsCopyUUIDForDevice(dev: libc::dev_t) -> *const c_void;
    fn FSEventStreamCreate(
        alloc: *const c_void,
        callback: Callback,
        context: *mut Context,
        paths: *const c_void,
        since: u64,
        latency: f64,
        flags: u32,
    ) -> Stream;
    fn FSEventStreamSetDispatchQueue(stream: Stream, queue: *mut c_void);
    fn FSEventStreamStart(stream: Stream) -> u8;
    fn FSEventStreamFlushSync(stream: Stream);
    fn FSEventStreamStop(stream: Stream);
    fn FSEventStreamInvalidate(stream: Stream);
    fn FSEventStreamRelease(stream: Stream);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeArrayCallBacks: c_void;
    fn CFUUIDGetUUIDBytes(uuid: *const c_void) -> UuidBytes;
    fn CFStringCreateWithBytes(
        alloc: *const c_void,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: u8,
    ) -> *const c_void;
    fn CFArrayCreate(
        alloc: *const c_void,
        values: *const *const c_void,
        n: isize,
        callbacks: *const c_void,
    ) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

unsafe extern "C" {
    fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> *mut c_void;
    fn dispatch_release(object: *mut c_void);
}

pub fn current_id() -> u64 {
    // SAFETY: no arguments, no state
    unsafe { FSEventsGetCurrentEventId() }
}

/// The id of the volume's event store: it changes when the volume's history is reset.
pub fn volume_uuid(dev: u64) -> Option<u128> {
    // SAFETY: returns an owned CFUUID or NULL (no history kept for that volume)
    unsafe {
        let uuid = FSEventsCopyUUIDForDevice(dev as libc::dev_t);
        if uuid.is_null() {
            return None;
        }
        let bytes = CFUUIDGetUUIDBytes(uuid);
        CFRelease(uuid);
        Some(u128::from_be_bytes(bytes.0))
    }
}

#[derive(Default)]
struct Found {
    events: Vec<(PathBuf, u64)>,
    broken: bool,
    /// history replayed
    done: bool,
    /// the marker's event arrived
    marked: bool,
}

struct State {
    found: Mutex<Found>,
    done: Condvar,
    /// the marker's directory, and the last event id before the marker was written
    mark_dir: PathBuf,
    before_mark: u64,
}

extern "C" fn callback(
    _stream: Stream,
    info: *mut c_void,
    n: usize,
    paths: *mut c_void,
    flags: *const u32,
    ids: *const u64,
) {
    // SAFETY: `info` is the `State` `changes` keeps alive until the stream is invalidated;
    // without kFSEventStreamCreateFlagUseCFTypes `paths` is `n` C strings, and `flags` and
    // `ids` hold `n` values each
    let (state, paths, flags, ids) = unsafe {
        (
            &*(info as *const State),
            std::slice::from_raw_parts(paths as *const *const c_char, n),
            std::slice::from_raw_parts(flags, n),
            std::slice::from_raw_parts(ids, n),
        )
    };
    let mut found = state.found.lock().unwrap();
    for i in 0..n {
        if flags[i] & HISTORY_DONE != 0 {
            found.done = true;
            state.done.notify_all();
            continue;
        }
        found.broken |= breaks_history(flags[i]);
        // SAFETY: a NUL-terminated path, valid during the call
        let path = unsafe { CStr::from_ptr(paths[i]) };
        let path = Path::new(std::ffi::OsStr::from_bytes(path.to_bytes()));
        if ids[i] > state.before_mark && path == state.mark_dir {
            found.marked = true;
            state.done.notify_all();
        }
        found.events.push((path.to_path_buf(), ids[i]));
    }
}

/// The directories under `roots` that changed after event `since`, each with its last
/// event id. `None` when the history cannot tell (see `breaks_history`) or does not come.
///
/// Events reach the history a moment after the change. So that a change made just before
/// the call is not missed, the file `marker` is written once the stream runs, and the
/// history is complete when its event arrives (the kernel queues events in order).
pub fn changes(roots: &[PathBuf], since: u64, marker: &Path) -> Option<Vec<(PathBuf, u64)>> {
    let mark_dir = std::fs::canonicalize(marker.parent()?).ok()?;
    let mut watched = roots.to_vec();
    watched.push(mark_dir.clone());
    let state = State {
        found: Mutex::default(),
        done: Condvar::new(),
        mark_dir,
        before_mark: current_id(),
    };
    let mut context = Context {
        version: 0,
        info: &state as *const State as *mut c_void,
        retain: std::ptr::null(),
        release: std::ptr::null(),
        copy_description: std::ptr::null(),
    };
    // SAFETY: CF objects are created and released here; the stream is invalidated (no more
    // callbacks) before `state` and `context` go out of scope
    let timed_out = unsafe {
        let strings: Vec<*const c_void> = watched
            .iter()
            .map(|r| {
                let b = r.as_os_str().as_bytes();
                CFStringCreateWithBytes(
                    std::ptr::null(),
                    b.as_ptr(),
                    b.len() as isize,
                    0x0800_0100,
                    0,
                )
            })
            .filter(|s| !s.is_null())
            .collect();
        let paths = CFArrayCreate(
            std::ptr::null(),
            strings.as_ptr(),
            strings.len() as isize,
            &kCFTypeArrayCallBacks,
        );
        strings.iter().for_each(|s| CFRelease(*s));
        if paths.is_null() {
            return None;
        }
        let stream = FSEventStreamCreate(
            std::ptr::null(),
            callback,
            &mut context,
            paths,
            since,
            0.0,
            CREATE_NO_DEFER,
        );
        CFRelease(paths);
        if stream.is_null() {
            return None;
        }
        let queue = dispatch_queue_create(c"oc.fsevents".as_ptr(), std::ptr::null());
        FSEventStreamSetDispatchQueue(stream, queue);
        let started = FSEventStreamStart(stream) != 0;
        let timed_out = !started || std::fs::write(marker, b"").is_err() || {
            // the lock is released at once: the callback takes it during the flush
            let found = state.found.lock().unwrap();
            let waited = state
                .done
                .wait_timeout_while(found, Duration::from_secs(10), |f| !(f.done && f.marked))
                .unwrap();
            waited.1.timed_out()
        };
        if started {
            FSEventStreamFlushSync(stream);
            FSEventStreamStop(stream);
        }
        FSEventStreamInvalidate(stream);
        FSEventStreamRelease(stream);
        dispatch_release(queue);
        timed_out
    };
    let found = state.found.into_inner().unwrap();
    (!timed_out && !found.broken).then_some(found.events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_breaks_on_lost_or_coarse_events() {
        assert!(!breaks_history(0));
        assert!(!breaks_history(0x100 | 0x800)); // item flags
        for f in [
            MUST_SCAN_SUB_DIRS,
            USER_DROPPED,
            KERNEL_DROPPED,
            EVENT_IDS_WRAPPED,
            ROOT_CHANGED,
            MOUNT,
            UNMOUNT,
        ] {
            assert!(breaks_history(f | 0x100), "{f:#x}");
        }
    }
}
