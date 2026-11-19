//! Item lists the user picks from (Uninstaller, Installers, Project Junk): found and
//! measured first, removed only when the user confirms, through the same checks as System
//! Junk: guards for the item's purpose, then "still the same object", then the action.

use crate::context::Context;
use crate::fs::Kind;
use crate::guard::{self, Identity, Purpose};
use crate::ledger::{self, Line};
use crate::procs::Running;
use crate::trash;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// To the Trash / Recycle Bin: the user can put it back.
    Trash,
    /// Gone for good (rebuildable things only).
    Delete,
    /// Slim Apps: thin a universal app to its arm64 code (the original goes to the Trash).
    Slim,
}

/// One thing the user can pick.
#[derive(Clone, Debug, Serialize)]
pub struct Pick {
    pub path: PathBuf,
    pub name: String,
    /// What it belongs to (an app name, a project folder…), for grouping in the list.
    pub group: String,
    /// Short explanation ("Preferences", "older than 30 days"…).
    pub detail: String,
    pub bytes: u64,
    pub preselect: bool,
    pub action: Action,
    #[serde(skip)]
    pub root: PathBuf,
    #[serde(skip)]
    pub id: Option<Identity>,
    #[serde(skip)]
    pub purpose: Option<Purpose>,
    /// Refuse while one of these apps runs (bundle ids / exe names).
    #[serde(skip)]
    pub running: Vec<String>,
}

impl Pick {
    pub fn new(path: PathBuf, root: PathBuf, purpose: Purpose, action: Action) -> Pick {
        Pick {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            id: guard::identity(&path),
            path,
            root,
            group: String::new(),
            detail: String::new(),
            bytes: 0,
            preselect: false,
            action,
            purpose: Some(purpose),
            running: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct PickList {
    pub plan: u64,
    pub items: Vec<Pick>,
    pub bytes: u64,
}

struct Stored {
    id: u64,
    command: String,
    picks: Vec<Pick>,
}

static STORE: Mutex<Vec<Stored>> = Mutex::new(Vec::new());
static NEXT: AtomicU64 = AtomicU64::new(1_000_000);
const MAX_STORED: usize = 32;

/// Keeps the list for a later `execute`, dropping items the guards refuse now.
pub fn store(command: &str, picks: Vec<Pick>, ctx: &Context) -> PickList {
    let picks: Vec<Pick> = picks
        .into_iter()
        .filter(|p| {
            p.id.is_some()
                && p.purpose
                    .is_some_and(|purpose| guard::check_for(&p.path, &p.root, ctx, purpose).is_ok())
        })
        .collect();
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let bytes = picks.iter().map(|p| p.bytes).sum();
    let list = PickList {
        plan: id,
        items: picks.clone(),
        bytes,
    };
    let mut store = STORE.lock().unwrap();
    store.push(Stored {
        id,
        command: command.into(),
        picks,
    });
    if store.len() > MAX_STORED {
        store.remove(0);
    }
    list
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Outcome {
    pub removed: u64,
    pub bytes: u64,
    pub skipped: u64,
    pub failed: u64,
    pub first_failure: Option<String>,
    /// Per item, in request order: "removed", "skipped: why" or "failed: why".
    pub results: Vec<String>,
    pub cancelled: bool,
    /// The item being worked on now (progress events only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<PathBuf>,
}

/// Acts on the items at `indexes` of list `plan`, each re-checked right before.
pub fn execute(
    ctx: &Context,
    plan: u64,
    indexes: &[usize],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Outcome, u64),
) -> Result<Outcome, String> {
    let (command, picks) = {
        let store = STORE.lock().unwrap();
        let s = store
            .iter()
            .find(|s| s.id == plan)
            .ok_or("this list has expired; look again")?;
        (s.command.clone(), s.picks.clone())
    };
    let running = Running::snapshot();
    let mut ledger = ledger::Writer::open(ctx);
    let session = ledger::now();
    let mut out = Outcome::default();
    // the first item is reported at once: a big folder can take a while
    let mut last = Instant::now() - Duration::from_secs(1);
    let total = indexes.len() as u64;
    for &i in indexes {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        let Some(pick) = picks.get(i) else {
            out.results.push("skipped: unknown item".into());
            out.skipped += 1;
            continue;
        };
        if last.elapsed() >= Duration::from_millis(150) {
            out.current = Some(pick.path.clone());
            progress(&out, total);
            last = Instant::now();
        }
        let result = act(pick, &running, ctx);
        let (word, reason) = match &result {
            Ok(()) => {
                out.removed += 1;
                out.bytes += pick.bytes;
                ("removed", None)
            }
            Err(Failure::Skipped(why)) => {
                out.skipped += 1;
                ("skipped", Some(why.clone()))
            }
            Err(Failure::Failed(why)) => {
                out.failed += 1;
                out.first_failure
                    .get_or_insert_with(|| format!("{}: {why}", pick.path.display()));
                ("failed", Some(why.clone()))
            }
        };
        out.results.push(match &reason {
            Some(r) => format!("{word}: {r}"),
            None => word.into(),
        });
        ledger.write(&Line::Item {
            session,
            rule: command.clone(),
            path: pick.path.clone(),
            bytes: if word == "removed" { pick.bytes } else { 0 },
            result: word.into(),
            reason,
        });
    }
    out.current = None;
    ledger.write(&Line::Session {
        id: session,
        command,
        started: session,
        finished: ledger::now(),
        items: out.removed,
        bytes: out.bytes,
        failed: out.failed,
    });
    Ok(out)
}

enum Failure {
    Skipped(String),
    Failed(String),
}

fn act(pick: &Pick, running: &Running, ctx: &Context) -> Result<(), Failure> {
    let (Some(purpose), Some(id)) = (pick.purpose, pick.id) else {
        return Err(Failure::Skipped("not checked".into()));
    };
    if let Some(app) = running.any(&pick.running) {
        return Err(Failure::Skipped(format!("{app} is running")));
    }
    guard::check_for(&pick.path, &pick.root, ctx, purpose)
        .map_err(|why| Failure::Skipped(why.into()))?;
    if !guard::still_same(&pick.path, &id) {
        return Err(Failure::Skipped("changed since it was listed".into()));
    }
    let result = match pick.action {
        Action::Trash => trash::move_to_trash(&pick.path),
        Action::Delete => remove(&pick.path, id.kind),
        Action::Slim => slim(&pick.path),
    };
    result.map_err(|e| Failure::Failed(e.to_string()))
}

fn remove(path: &Path, kind: Kind) -> std::io::Result<()> {
    match kind {
        Kind::Dir => std::fs::remove_dir_all(path),
        _ => std::fs::remove_file(path),
    }
}

#[cfg(target_os = "macos")]
fn slim(path: &Path) -> std::io::Result<()> {
    crate::slim::slim_app(path)
}

#[cfg(not(target_os = "macos"))]
fn slim(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}
