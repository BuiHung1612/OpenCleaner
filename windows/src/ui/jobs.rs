//! Engine requests off the UI thread. Each job runs `oc_engine::api::run` on its own thread
//! and posts every event to the window (`WM_ENGINE`); the window owns the event from there.

use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_ENGINE: u32 = WM_APP + 1;

/// A running job: its id comes back with every event; `cancel` stops it.
pub struct Job {
    pub id: u64,
    cancel: Arc<AtomicBool>,
}

impl Job {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// An event as the window receives it: (job id, event JSON).
pub type Posted = (u64, Value);

static NEXT: AtomicU64 = AtomicU64::new(1);

pub fn start(hwnd: HWND, request: Value) -> Job {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let target = hwnd as isize;
    std::thread::spawn(move || {
        oc_engine::api::run(&request.to_string(), &flag, &mut |event| {
            let boxed = Box::into_raw(Box::new((id, event) as Posted));
            // the window frees it; if posting fails (window gone), free it here
            if unsafe { PostMessageW(target as HWND, WM_ENGINE, 0, boxed as isize) } == 0 {
                drop(unsafe { Box::from_raw(boxed) });
            }
        });
    });
    Job { id, cancel }
}

/// Quick request, answered on the calling thread.
pub fn call(request: Value) -> Value {
    oc_engine::api::call(&request.to_string())
}
