//! Requests the app sends, as JSON: quick ones answered at once (`call`), long ones run
//! as jobs that report events until `{"event":"done"}` (`run`).

use crate::context::Context;
use crate::rules::RuleSet;
use crate::{apps, clean, finders, ledger, maintenance, picks, procs, size, space, status};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::AtomicBool;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Request {
    Version,
    /// Dashboard reading (CPU since the previous reading).
    Status,
    Size {
        paths: Vec<PathBuf>,
        #[serde(default = "allocated")]
        mode: size::Mode,
        #[serde(default)]
        all_volumes: bool,
    },
    /// Space Lens: the children of `path` by size, or the startup disk overview.
    /// Emits `progress` listings while it runs, then one `space` listing.
    Space {
        path: Option<PathBuf>,
    },
    /// The cleaning rules and their groups, for the review screen.
    Rules,
    /// System Junk scan: `progress` summaries, then one `plan` (nothing is removed).
    /// `list` adds every item's path and size to its rule.
    Scan {
        #[serde(default)]
        list: bool,
    },
    /// Remove the items of `rules` from a scanned plan: `progress`, then `cleaned`.
    Clean {
        plan: u64,
        rules: Vec<String>,
    },
    /// Installed apps at once (`apps`), then their sizes as they are measured (`sizes`).
    Apps,
    /// What uninstalling these apps would move to the Trash: the bundles and their
    /// leftovers, as a pick list (`picks`).
    UninstallPlan {
        paths: Vec<PathBuf>,
    },
    /// Windows: starts the vendor's uninstaller of the app registered under `key`.
    RunUninstaller {
        key: String,
    },
    /// Installer files in Downloads and on the Desktop (`picks`).
    Installers,
    /// Rebuildable build folders in projects (`picks`).
    Projects {
        #[serde(default)]
        roots: Vec<PathBuf>,
    },
    /// Act on the picked `items` (indexes) of a pick list: `progress`, then `removed`.
    Remove {
        plan: u64,
        items: Vec<usize>,
    },
    /// Universal apps that Slim Apps can cut down to their arm64 code (`picks`).
    Slim,
    /// Optimization tasks and the Touch ID state.
    Maintenance,
    /// Runs the chosen tasks that need no password; returns the script for the others
    /// (`admin_script`, for the app to run with administrator rights).
    Optimize {
        tasks: Vec<String>,
    },
    /// The script that turns Touch ID for sudo on or off (needs administrator rights).
    TouchidScript {
        enable: bool,
    },
    /// Past cleanups, newest first.
    History {
        #[serde(default = "history_limit")]
        limit: usize,
    },
}

fn history_limit() -> usize {
    200
}

fn ctx() -> &'static Context {
    static CTX: OnceLock<Context> = OnceLock::new();
    CTX.get_or_init(Context::from_env)
}

fn rule_set() -> &'static RuleSet {
    static SET: OnceLock<RuleSet> = OnceLock::new();
    SET.get_or_init(RuleSet::builtin)
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
        Ok(Request::Status) => {
            let mut v = serde_json::to_value(status::read()).unwrap_or(Value::Null);
            v["ok"] = json!(true);
            v
        }
        Ok(Request::Rules) => {
            let set = rule_set();
            let groups: Vec<Value> = set
                .groups
                .iter()
                .map(|g| {
                    let rules: Vec<Value> = set
                        .rules
                        .iter()
                        .filter(|r| r.group == g.id)
                        .map(|r| json!({"id": r.id, "title": r.title, "note": r.note, "default": r.default}))
                        .collect();
                    json!({"id": g.id, "title": g.title, "rules": rules})
                })
                .collect();
            json!({"ok": true, "groups": groups})
        }
        Ok(Request::RunUninstaller { key }) => match apps::run_uninstaller(&key) {
            Ok(name) => {
                let mut ledger = ledger::Writer::open(ctx());
                let now = ledger::now();
                ledger.write(&ledger::Line::Session {
                    id: now,
                    command: "uninstall".into(),
                    started: now,
                    finished: now,
                    items: 1,
                    bytes: 0,
                    failed: 0,
                });
                json!({"ok": true, "started": name})
            }
            Err(e) => json!({"ok": false, "error": e}),
        },
        Ok(Request::Maintenance) => {
            json!({"ok": true, "tasks": maintenance::tasks(), "touchid": maintenance::touchid_status()})
        }
        Ok(Request::Optimize { tasks }) => json!({
            "ok": true,
            "results": maintenance::run_user(&tasks),
            "admin_script": maintenance::admin_script(&tasks),
        }),
        Ok(Request::TouchidScript { enable }) => {
            json!({"ok": true, "script": maintenance::touchid_script(enable)})
        }
        Ok(Request::History { limit }) => {
            let mut v = ledger::sessions(ctx(), limit);
            v["ok"] = json!(true);
            v
        }
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
        Request::Status => {
            let mut v = serde_json::to_value(status::read()).unwrap_or(Value::Null);
            v["event"] = json!("status");
            emit(v);
        }
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
        Request::Rules
        | Request::History { .. }
        | Request::Maintenance
        | Request::RunUninstaller { .. }
        | Request::Optimize { .. }
        | Request::TouchidScript { .. } => emit(json!({"event": "error", "error": "use call"})),
        Request::Apps => {
            let list = apps::list(ctx(), &procs::Running::snapshot());
            emit(json!({"event": "apps", "apps": list}));
            let paths: Vec<PathBuf> = list.iter().map(|a| a.path.clone()).collect();
            let mut progress =
                |sizes: &[u64]| emit(json!({"event": "sizes", "sizes": sizes, "final": false}));
            let sizes = apps::sizes(ctx(), &paths, cancel, &mut progress);
            emit(json!({"event": "sizes", "sizes": sizes, "final": true}));
        }
        Request::UninstallPlan { paths } => {
            let running = procs::Running::snapshot();
            let known = apps::list(ctx(), &running);
            let mut all = Vec::new();
            for path in paths {
                match known.iter().find(|a| a.path == path) {
                    Some(app) => all.extend(apps::uninstall_picks(app, ctx(), cancel)),
                    None => emit(
                        json!({"event": "error", "path": path, "error": "not an installed app OpenCleaner can remove"}),
                    ),
                }
            }
            emit_value("picks", &picks::store("uninstall", all, ctx()), emit);
        }
        Request::Slim => {
            #[cfg(target_os = "macos")]
            let found = crate::slim::candidates(&apps::list(ctx(), &procs::Running::snapshot()));
            #[cfg(not(target_os = "macos"))]
            let found = Vec::new();
            emit_value("picks", &picks::store("slim", found, ctx()), emit);
        }
        Request::Installers => {
            let found = finders::installers(ctx());
            emit_value("picks", &picks::store("installers", found, ctx()), emit);
        }
        Request::Projects { roots } => {
            let found = finders::projects(ctx(), &roots, cancel);
            emit_value("picks", &picks::store("projects", found, ctx()), emit);
        }
        Request::Remove { plan, items } => {
            let mut progress = |o: &picks::Outcome, total: u64| {
                let mut v = serde_json::to_value(o).unwrap_or(Value::Null);
                v["event"] = json!("progress");
                v["total"] = json!(total);
                emit(v);
            };
            match picks::execute(ctx(), plan, &items, cancel, &mut progress) {
                Ok(outcome) => emit_value("removed", &outcome, emit),
                Err(e) => emit(json!({"event": "removed", "error": e})),
            }
        }
        Request::Scan { list } => {
            let t = std::time::Instant::now();
            let running = procs::Running::snapshot();
            if std::env::var_os("OC_TRACE").is_some() {
                eprintln!("scan: running apps in {:?}", t.elapsed());
            }
            let mut progress = |p: &clean::PlanSummary| emit_value("progress", p, &mut *emit);
            let plan = clean::scan_with(ctx(), rule_set(), &running, list, cancel, &mut progress);
            emit_value("plan", &plan, emit);
        }
        Request::Clean { plan, rules } => {
            let mut progress = |o: &clean::Outcome, total: u64| {
                let mut v = serde_json::to_value(o).unwrap_or(Value::Null);
                v["event"] = json!("progress");
                v["total"] = json!(total);
                emit(v);
            };
            match clean::execute(ctx(), plan, &rules, cancel, &mut progress) {
                Ok(outcome) => emit_value("cleaned", &outcome, emit),
                Err(e) => emit(json!({"event": "cleaned", "error": e})),
            }
        }
        Request::Space { path } => {
            let mut progress = |l: &space::Listing| emit_listing("progress", l, &mut *emit);
            let result = match &path {
                Some(p) => space::list(p, cancel, &mut progress),
                None => space::overview(cancel, &mut progress),
            };
            match result {
                Ok(l) => emit_listing("space", &l, emit),
                Err(e) => emit(json!({"event": "space", "path": path, "error": e.to_string()})),
            }
        }
    }
}

fn emit_listing(event: &str, listing: &space::Listing, emit: &mut dyn FnMut(Value)) {
    let mut v = serde_json::to_value(listing).unwrap_or(Value::Null);
    v["event"] = json!(event);
    emit(v);
}

fn emit_value<T: serde::Serialize>(event: &str, value: &T, emit: &mut dyn FnMut(Value)) {
    let mut v = serde_json::to_value(value).unwrap_or(Value::Null);
    v["event"] = json!(event);
    emit(v);
}
