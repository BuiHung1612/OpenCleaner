//! Requests the app sends, as JSON: quick ones answered at once (`call`), long ones run
//! as jobs that report events until `{"event":"done"}` (`run`).

use crate::size;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Version,
    Size {
        paths: Vec<PathBuf>,
        #[serde(default = "allocated")]
        mode: size::Mode,
        #[serde(default)]
        all_volumes: bool,
    },
}

fn allocated() -> size::Mode {
    size::Mode::Allocated
}

fn parse(request: &str) -> Result<Request, Value> {
    serde_json::from_str(request)
        .map_err(|e| json!({"ok": false, "error": format!("bad request: {e}")}))
}

/// Answers a quick request.
pub fn call(request: &str) -> Value {
    match parse(request) {
        Err(e) => e,
        Ok(Request::Version) => json!({"ok": true, "version": crate::VERSION}),
        Ok(req) => {
            let mut out = Vec::new();
            run_request(req, &AtomicBool::new(false), &mut |v| out.push(v));
            json!({"ok": true, "events": out})
        }
    }
}

/// Runs a request as a job, reporting each event; the last one is `{"event":"done"}`.
pub fn run(request: &str, cancel: &AtomicBool, emit: &mut dyn FnMut(Value)) {
    match parse(request) {
        Err(e) => emit(e),
        Ok(req) => run_request(req, cancel, emit),
    }
    emit(json!({"event": "done"}));
}

fn run_request(req: Request, cancel: &AtomicBool, emit: &mut dyn FnMut(Value)) {
    match req {
        Request::Version => emit(json!({"event": "version", "version": crate::VERSION})),
        Request::Size {
            paths,
            mode,
            all_volumes,
        } => {
            let opts = size::Options {
                mode,
                one_volume: !all_volumes,
                ..Default::default()
            };
            let links = size::Links::default();
            for path in paths {
                match size::measure_with(&path, &opts, &links, cancel) {
                    Ok(r) => emit(json!({"event": "size", "path": path, "report": r})),
                    Err(e) => emit(json!({"event": "size", "path": path, "error": e.to_string()})),
                }
            }
        }
    }
}
