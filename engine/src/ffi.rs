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

/// Starts a job on its own thread and returns its id. `callback` gets every event, from
/// that thread, ending with `{"event":"done"}`.
///
/// # Safety
/// `engine` is live until the job is done; `ctx` stays valid until the "done" event.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oc_start(
    engine: *mut OcEngine,
    request_json: *const c_char,
    callback: OcEventFn,
    ctx: *mut c_void,
) -> u64 {
    let engine = unsafe { &*engine };
    let request = unsafe { str_arg(request_json) }.to_owned();
    let id = engine.next_job.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    engine.jobs.lock().unwrap().insert(id, cancel.clone());
    let ctx = Ctx(ctx);
    let engine_addr = engine as *const OcEngine as usize;
    std::thread::spawn(move || {
        let ctx = ctx;
        api::run(&request, &cancel, &mut |event| {
            let c = CString::new(event.to_string().replace('\0', "")).unwrap();
            callback(ctx.0, c.as_ptr());
        });
        let engine = unsafe { &*(engine_addr as *const OcEngine) };
        engine.jobs.lock().unwrap().remove(&id);
    });
    id
}

/// Asks a running job to stop; it still ends with a "done" event.
///
/// # Safety
/// `engine` comes from `oc_engine_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn oc_cancel(engine: *mut OcEngine, job: u64) {
    let engine = unsafe { &*engine };
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
