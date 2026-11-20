//! Item lists the user picks from (Uninstaller, Installers, Project Junk): found and
//! measured first, removed only when the user confirms, through the same checks as System
//! Junk: guards for the item's purpose, then "still the same object", then the action.

use crate::context::Context;
use crate::fs::Kind;
use crate::guard::{self, Identity, Purpose, Stamp};
use crate::ledger::{self, Line};
use crate::procs::Running;
use crate::trash;
use serde::Serialize;
use std::collections::HashMap;
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
    /// The parts of `detail`, so an app can word it in its own language (all optional):
    /// what it is ("PKG", "screenshot", "npm packages"…), its age or time unopened in days,
    /// where it sits, how many copies its set has, and why it is not preselected.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_days: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_days: Option<i64>,
    /// Hidden home items: days since it or what it directly holds last changed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changed_days: Option<i64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub place: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copies: Option<usize>,
    /// "active" (touched this week), "git_unknown", "kept" (the copy that stays)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flags: Vec<&'static str>,
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
    /// A ticked child of this listed folder: the purpose checks (a build folder, an
    /// installer…) apply to the folder, the child only has to sit directly in it.
    #[serde(skip)]
    pub within: Option<PathBuf>,
    /// The user's files: length and mtime too, so a file edited since it was listed (same
    /// dev/ino) is left alone.
    #[serde(skip)]
    pub stamp: Option<Stamp>,
    /// Duplicates: which set of identical copies it belongs to. A set is acted on only
    /// while one of its copies stays, unchanged.
    #[serde(skip)]
    pub set: Option<usize>,
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
            kind: String::new(),
            age_days: None,
            idle_days: None,
            changed_days: None,
            place: String::new(),
            copies: None,
            flags: Vec::new(),
            bytes: 0,
            preselect: false,
            action,
            purpose: Some(purpose),
            running: Vec::new(),
            within: None,
            stamp: None,
            set: None,
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
    /// Per item acted on (a ticked part counts as one), in request order: "removed",
    /// "skipped: why" or "failed: why".
    pub results: Vec<String>,
    pub cancelled: bool,
    /// The item being worked on now (progress events only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<PathBuf>,
}

/// Acts on the items at `indexes` of list `plan`, each re-checked right before. An index
/// in `parts` acts only on those children of its folder (the user opened it and ticked some).
pub fn execute(
    ctx: &Context,
    plan: u64,
    indexes: &[usize],
    parts: &HashMap<usize, Vec<PathBuf>>,
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
    // what to act on: whole picks, or the ticked children of a pick (refused up front
    // when they are not its children any more)
    let mut work: Vec<(Pick, Option<String>)> = Vec::new();
    // Duplicates: does each set keep a copy that is not picked and still as listed?
    let picked: std::collections::HashSet<usize> = indexes.iter().copied().collect();
    let mut keeps: HashMap<usize, bool> = HashMap::new();
    for &i in indexes {
        let Some(pick) = picks.get(i) else {
            out.results.push("skipped: unknown item".into());
            out.skipped += 1;
            continue;
        };
        if let Some(set) = pick.set
            && !*keeps.entry(set).or_insert_with(|| {
                picks
                    .iter()
                    .enumerate()
                    .any(|(j, o)| o.set == Some(set) && !picked.contains(&j) && unchanged(o))
            })
        {
            work.push((pick.clone(), Some("no copy would be left".into())));
            continue;
        }
        let Some(children) = parts.get(&i) else {
            work.push((pick.clone(), None));
            continue;
        };
        for child in children {
            let mut part = pick.clone();
            part.path = child.clone();
            part.within = Some(pick.path.clone());
            part.name = child
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let checked = pick
                .id
                .ok_or_else(|| "not checked".to_string())
                .and_then(|id| guard::part_of(&pick.path, &id, child));
            match checked {
                Ok(id) => {
                    part.id = Some(id);
                    part.bytes = guard::freed_now(child);
                    work.push((part, None));
                }
                Err(why) => work.push((part, Some(why))),
            }
        }
    }
    let total = work.len() as u64;
    for (pick, refused) in &work {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        if last.elapsed() >= Duration::from_millis(150) {
            out.current = Some(pick.path.clone());
            progress(&out, total);
            last = Instant::now();
        }
        let result = match refused {
            Some(why) => Err(Failure::Skipped(why.clone())),
            None => act(pick, &running, ctx),
        };
        // what it freed: measured for Slim (only part of the Intel code may go), else the listing's size
        let freed = match &result {
            Ok(done) => done.freed.unwrap_or(pick.bytes),
            Err(_) => 0,
        };
        let (word, reason) = match &result {
            Ok(_) => {
                out.removed += 1;
                out.bytes += freed;
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
        let trashed = result.ok().and_then(|done| done.trashed);
        ledger.write(&Line::Item {
            session,
            rule: command.clone(),
            path: pick.path.clone(),
            bytes: freed,
            result: word.into(),
            reason,
            stamp: trashed.as_deref().and_then(guard::stamp),
            trashed,
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

/// What acting on one pick did.
struct Done {
    /// where the item went in the Trash (when it went there and the system said)
    trashed: Option<PathBuf>,
    /// bytes freed, when measured rather than the listing's estimate
    freed: Option<u64>,
}

fn act(pick: &Pick, running: &Running, ctx: &Context) -> Result<Done, Failure> {
    let (Some(purpose), Some(id)) = (pick.purpose, pick.id) else {
        return Err(Failure::Skipped("not checked".into()));
    };
    if let Some(app) = running.any(&pick.running) {
        return Err(Failure::Skipped(format!("{app} is running")));
    }
    guard::check_for(
        pick.within.as_ref().unwrap_or(&pick.path),
        &pick.root,
        ctx,
        purpose,
    )
    .map_err(|why| Failure::Skipped(why.into()))?;
    if !unchanged(pick) {
        return Err(Failure::Skipped("changed since it was listed".into()));
    }
    let done = |trashed| Done {
        trashed,
        freed: None,
    };
    let result = match pick.action {
        Action::Trash => trash::move_to_trash(&pick.path).map(done),
        Action::Delete => remove(&pick.path, id.kind).map(|()| done(None)),
        Action::Slim => slim(&pick.path).map(|(freed, trashed)| Done {
            trashed,
            freed: Some(freed),
        }),
    };
    result.map_err(|e| Failure::Failed(e.to_string()))
}

/// Still the object that was listed (and, for the user's files, the same length and mtime).
fn unchanged(pick: &Pick) -> bool {
    pick.id.is_some_and(|id| guard::still_same(&pick.path, &id))
        && (pick.stamp.is_none() || guard::stamp(&pick.path) == pick.stamp)
}

fn remove(path: &Path, kind: Kind) -> std::io::Result<()> {
    match kind {
        Kind::Dir => std::fs::remove_dir_all(path),
        _ => std::fs::remove_file(path),
    }
}

#[cfg(target_os = "macos")]
fn slim(path: &Path) -> std::io::Result<(u64, Option<PathBuf>)> {
    crate::slim::slim_app(path)
}

#[cfg(not(target_os = "macos"))]
fn slim(_path: &Path) -> std::io::Result<(u64, Option<PathBuf>)> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}
