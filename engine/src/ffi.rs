//! C ABI for the app (header: `include/oc_engine.h`). Requests and events are UTF-8 JSON.
//! Strings returned by the engine are freed with `oc_free`; event strings passed to the
//! callback are only valid during the call.

use crate::api;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub type OcEventFn = extern "C" fn(ctx: *mut c_void, event_json: *const c_char);

pub struct OcEngine {
    next_job: AtomicU64,
    jobs: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

/// The callback context crosses to the job thread; the caller keeps it alive until "done".
struct Ctx(*mut c_void);
unsafe impl Send for Ctx {}

fn to_c(s: String) -> *mut c_char {
    CString::new(s.replace('\0', "")).unwrap().into_raw()
}

unsafe fn str_arg<'a>(p: *const c_char) -> &'a str {
    if p.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn oc_engine_new(_config_json: *const c_char) -> *mut OcEngine {
    Box::into_raw(Box::new(OcEngine {
        next_job: AtomicU64::new(1),
        jobs: Mutex::new(HashMap::new()),
    }))
}

/// # Safety
/// `engine` comes from `oc_engine_new` and no job is still running.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oc_engine_free(engine: *mut OcEngine) {
    if !engine.is_null() {
        drop(unsafe { Box::from_raw(engine) });
    }
}

/// Answers a quick request; free the result with `oc_free`.
///
/// # Safety
/// `request_json` is a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oc_call(
    _engine: *mut OcEngine,
    request_json: *const c_char,
) -> *mut c_char {
    to_c(api::call(unsafe { str_arg(request_json) }).to_string())
}

/// Starts a job on its own thread and returns its id (0 when `engine` is null). `callback`
/// gets every event, from that thread, ending with `{"event":"done"}`. The engine is not
/// touched after "done", so the host may free it then. A panic in the job (debug builds;
/// release aborts) still ends with an error event and "done".
///
/// # Safety
/// `engine` is live until the "done" event; `ctx` stays valid until then.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oc_start(
    engine: *mut OcEngine,
    request_json: *const c_char,
    callback: OcEventFn,
    ctx: *mut c_void,
) -> u64 {
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return 0;
    };
    let request = unsafe { str_arg(request_json) }.to_owned();
    let id = engine.next_job.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    engine.jobs.lock().unwrap().insert(id, cancel.clone());
    let ctx = Ctx(ctx);
    let engine_addr = engine as *const OcEngine as usize;
    std::thread::spawn(move || {
        let ctx = ctx;
        let mut done = false;
        let mut emit = |event: serde_json::Value| {
            if done {
                return;
            }
            if event["event"] == "done" {
                // last use of the engine: before "done", after which the host may free it
                let engine = unsafe { &*(engine_addr as *const OcEngine) };
                engine
                    .jobs
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                done = true;
            }
            let c = CString::new(event.to_string().replace('\0', "")).unwrap();
            callback(ctx.0, c.as_ptr());
        };
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            api::run(&request, &cancel, &mut emit)
        }));
        if run.is_err() {
            emit(serde_json::json!({"event": "error", "error": "internal error"}));
            emit(serde_json::json!({"event": "done"}));
        }
    });
    id
}

/// Asks a running job to stop; it still ends with a "done" event. No-op on a null engine.
///
/// # Safety
/// `engine` comes from `oc_engine_new` (or is null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oc_cancel(engine: *mut OcEngine, job: u64) {
    let Some(engine) = (unsafe { engine.as_ref() }) else {
        return;
    };
    if let Some(flag) = engine.jobs.lock().unwrap().get(&job) {
        flag.store(true, Ordering::Relaxed);
    }
}

/// # Safety
/// `s` was returned by `oc_call` (or is null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oc_free(s: *mut c_char) {
    if !s.is_null() {
        drop(unsafe { CString::from_raw(s) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Sender, channel};

    struct Probe {
        engine: *mut OcEngine,
        tx: Sender<(String, usize)>,
    }

    extern "C" fn on_event(ctx: *mut c_void, event: *const c_char) {
        let probe = unsafe { &*(ctx as *const Probe) };
        let text = unsafe { CStr::from_ptr(event) }
            .to_string_lossy()
            .into_owned();
        let jobs = unsafe { &*probe.engine }.jobs.lock().unwrap().len();
        probe.tx.send((text, jobs)).unwrap();
    }

    #[test]
    fn job_is_gone_before_done_and_null_engine_is_refused() {
        unsafe {
            let null = std::ptr::null_mut();
            assert_eq!(
                oc_start(null, c"{}".as_ptr(), on_event, std::ptr::null_mut()),
                0
            );
            oc_cancel(null, 1);
        }
        let engine = oc_engine_new(std::ptr::null());
        let (tx, rx) = channel();
        let probe = Box::new(Probe { engine, tx });
        let ctx = &*probe as *const Probe as *mut c_void;
        let id = unsafe { oc_start(engine, c"{\"op\":\"version\"}".as_ptr(), on_event, ctx) };
        assert!(id > 0);
        let jobs_at_done = loop {
            let (e, jobs) = rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            if e.contains("\"done\"") {
                break jobs;
            }
        };
        assert_eq!(jobs_at_done, 0, "removed from the map before done");
        unsafe { oc_engine_free(engine) };
    }
}
