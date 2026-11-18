//! System Junk: scan the rules into a plan the user reviews, then remove what they kept
//! selected. Removal happens only here (`execute`), item by item, each one re-checked
//! right before it goes and written to the history.

use crate::context::Context;
use crate::fs::Kind;
use crate::guard::{self, Identity};
use crate::ledger::{self, Line};
use crate::procs::Running;
use crate::rules::{self, Rule, RuleSet, Target, is_within, matches_any};
use crate::size;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

#[derive(Clone, Debug)]
struct Item {
    rule: usize,
    path: PathBuf,
    root: PathBuf,
    id: Identity,
    bytes: u64,
}

/// One rule as the review shows it.
#[derive(Clone, Debug, Serialize)]
pub struct RuleSummary {
    pub id: String,
    pub group: String,
    pub title: String,
    pub note: String,
    pub default: bool,
    pub items: u64,
    pub bytes: u64,
    /// "ready", "empty", or "in_use" (an app it belongs to is running)
    pub status: &'static str,
    pub in_use_by: Option<String>,
    /// items kept because an app owning them runs, or a guard refused them
    pub kept: u64,
    /// The items themselves, when the scan was asked to list them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paths: Option<Vec<(PathBuf, u64)>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GroupSummary {
    pub id: String,
    pub title: String,
    pub rules: Vec<RuleSummary>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanSummary {
    pub plan: u64,
    pub groups: Vec<GroupSummary>,
    pub items: u64,
    pub bytes: u64,
    pub cancelled: bool,
}

struct Plan {
    id: u64,
    rules: Vec<Rule>,
    items: Vec<Item>,
}

static PLANS: Mutex<Vec<Plan>> = Mutex::new(Vec::new());
const MAX_PLANS: usize = 32;
static NEXT_PLAN: AtomicU64 = AtomicU64::new(1);

/// Finds and measures everything the rules point at. Nothing is removed.
pub fn scan(
    ctx: &Context,
    set: &RuleSet,
    running: &Running,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&PlanSummary),
) -> PlanSummary {
    scan_with(ctx, set, running, false, cancel, progress)
}

/// `scan`, optionally listing every item (path and freed bytes) in its rule's summary.
pub fn scan_with(
    ctx: &Context,
    set: &RuleSet,
    running: &Running,
    list: bool,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&PlanSummary),
) -> PlanSummary {
    let mut summaries: Vec<RuleSummary> = set
        .rules
        .iter()
        .map(|r| RuleSummary {
            id: r.id.clone(),
            group: r.group.clone(),
            title: r.title.clone(),
            note: r.note.clone(),
            default: r.default,
            items: 0,
            bytes: 0,
            status: "empty",
            in_use_by: None,
            kept: 0,
            paths: None,
        })
        .collect();

    // candidates in rule order: an earlier (more specific) rule claims a path first
    let mut items: Vec<Item> = Vec::new();
    let mut claimed: BTreeSet<PathBuf> = BTreeSet::new();
    for (ri, rule) in set.rules.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if let Some(app) = running.any(&rule.running) {
            summaries[ri].status = "in_use";
            summaries[ri].in_use_by = Some(app);
            continue;
        }
        for (path, root) in candidates(rule, ctx) {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if rule.item_is_bundle_id && running.contains(&name) {
                summaries[ri].kept += 1;
                continue;
            }
            if guard::check(&path, &root, ctx).is_err() || overlaps(&claimed, &path) {
                summaries[ri].kept += 1;
                continue;
            }
            let Some(id) = guard::identity(&path) else {
                continue;
            };
            if rule.min_age_days > 0 && !older_than(&path, rule.min_age_days) {
                continue;
            }
            claimed.insert(path.clone());
            items.push(Item {
                rule: ri,
                path,
                root,
                id,
                bytes: 0,
            });
        }
    }

    let paths: Vec<PathBuf> = items.iter().map(|i| i.path.clone()).collect();
    let mut emit_sizes = |sizes: &[u64]| {
        let mut partial = summaries.clone();
        tally(&mut partial, &items, sizes);
        progress(&summarize(0, set, partial, true, cancel));
    };
    let sizes = size::freed_many(&paths, cancel, &mut emit_sizes);
    for (item, b) in items.iter_mut().zip(&sizes) {
        item.bytes = *b;
    }
    // nothing to free: not worth a row (an empty cache folder)
    items.retain(|i| i.bytes > 0);
    tally(&mut summaries, &items, &sizes_of(&items));
    if list {
        for item in &items {
            summaries[item.rule]
                .paths
                .get_or_insert_with(Vec::new)
                .push((item.path.clone(), item.bytes));
        }
    }

    let id = NEXT_PLAN.fetch_add(1, Ordering::Relaxed);
    let summary = summarize(id, set, summaries, false, cancel);
    let mut plans = PLANS.lock().unwrap();
    plans.push(Plan {
        id,
        rules: set.rules.clone(),
        items,
    });
    // a plan stays valid while its review is open; other scans (other tools, other
    // windows) must not push it out, so keep plenty and drop the oldest
    if plans.len() > MAX_PLANS {
        plans.remove(0);
    }
    summary
}

fn sizes_of(items: &[Item]) -> Vec<u64> {
    items.iter().map(|i| i.bytes).collect()
}

fn tally(summaries: &mut [RuleSummary], items: &[Item], sizes: &[u64]) {
    for s in summaries.iter_mut() {
        s.items = 0;
        s.bytes = 0;
    }
    for (item, b) in items.iter().zip(sizes) {
        let s = &mut summaries[item.rule];
        s.items += 1;
        s.bytes += b;
    }
    for s in summaries.iter_mut() {
        if s.status != "in_use" {
            s.status = if s.items > 0 { "ready" } else { "empty" };
        }
    }
}

fn summarize(
    plan: u64,
    set: &RuleSet,
    rules: Vec<RuleSummary>,
    partial: bool,
    cancel: &AtomicBool,
) -> PlanSummary {
    let items = rules.iter().map(|r| r.items).sum();
    let bytes = rules.iter().map(|r| r.bytes).sum();
    let groups = set
        .groups
        .iter()
        .map(|g| GroupSummary {
            id: g.id.clone(),
            title: g.title.clone(),
            rules: rules
                .iter()
                .filter(|r| r.group == g.id && (partial || r.status != "empty"))
                .cloned()
                .collect(),
        })
        .filter(|g| !g.rules.is_empty())
        .collect();
    PlanSummary {
        plan,
        groups,
        items,
        bytes,
        cancelled: cancel.load(Ordering::Relaxed),
    }
}

/// The items a rule names, each with the root it was found under.
fn candidates(rule: &Rule, ctx: &Context) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for pattern in &rule.paths {
        for root in rules::expand(pattern, ctx) {
            match rule.target {
                Target::Itself => {
                    let name = root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if wanted(rule, &name) {
                        out.push((root.clone(), root));
                    }
                }
                Target::Contents => {
                    let Ok(meta) = std::fs::symlink_metadata(&root) else {
                        continue;
                    };
                    if !meta.is_dir() {
                        continue;
                    }
                    let Ok(children) = std::fs::read_dir(&root) else {
                        continue;
                    };
                    let mut names: Vec<PathBuf> = children
                        .flatten()
                        .filter(|c| c.file_name().to_str().is_some_and(|n| wanted(rule, n)))
                        .map(|c| c.path())
                        .collect();
                    names.sort();
                    out.extend(names.into_iter().map(|p| (p, root.clone())));
                }
            }
        }
    }
    out
}

fn wanted(rule: &Rule, name: &str) -> bool {
    (rule.include.is_empty() || matches_any(&rule.include, name))
        && !matches_any(&rule.exclude, name)
}

/// The path is inside, equal to, or contains something already claimed.
fn overlaps(claimed: &BTreeSet<PathBuf>, path: &Path) -> bool {
    if let Some(before) = claimed.range(..=path.to_path_buf()).next_back()
        && is_within(path, before)
    {
        return true;
    }
    claimed
        .range(path.to_path_buf()..)
        .next()
        .is_some_and(|after| is_within(after, path))
}

fn older_than(path: &Path, days: u32) -> bool {
    let Ok(modified) = std::fs::symlink_metadata(path).and_then(|m| m.modified()) else {
        return false;
    };
    SystemTime::now()
        .duration_since(modified)
        .is_ok_and(|age| age >= Duration::from_secs(u64::from(days) * 86_400))
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Outcome {
    pub removed: u64,
    pub bytes: u64,
    /// left alone: changed since the scan, now in use, or refused by a guard
    pub skipped: u64,
    pub failed: u64,
    pub first_failure: Option<String>,
    pub cancelled: bool,
}

/// Removes the items of `rule_ids` from plan `plan_id`. Each item is checked again first:
/// its app is not running now, the guards still accept the path, and it is still the
/// same file system object that was measured.
pub fn execute(
    ctx: &Context,
    plan_id: u64,
    rule_ids: &[String],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(&Outcome, u64),
) -> Result<Outcome, String> {
    let (rules, items) = {
        let plans = PLANS.lock().unwrap();
        let plan = plans
            .iter()
            .find(|p| p.id == plan_id)
            .ok_or("this scan has expired; scan again")?;
        (plan.rules.clone(), plan.items.clone())
    };
    let running = Running::snapshot();
    let selected: Vec<&Item> = items
        .iter()
        .filter(|i| rule_ids.contains(&rules[i.rule].id))
        .collect();
    let total = selected.len() as u64;

    let mut ledger = ledger::Writer::open(ctx);
    let session = ledger::now();
    let mut out = Outcome::default();
    let mut last = Instant::now();
    for item in selected {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        let rule = &rules[item.rule];
        let name = item
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let refusal = if let Some(app) = running.any(&rule.running) {
            Some(format!("{app} is running"))
        } else if rule.item_is_bundle_id && running.contains(&name) {
            Some(format!("{name} is running"))
        } else if let Err(why) = guard::check(&item.path, &item.root, ctx) {
            Some(why.to_string())
        } else if !guard::still_same(&item.path, &item.id) {
            Some("changed since the scan".to_string())
        } else {
            None
        };

        let (result, reason, freed) = match refusal {
            Some(why) => {
                out.skipped += 1;
                ("skipped", Some(why), 0)
            }
            None => match remove(&item.path, item.id.kind) {
                Ok(()) => {
                    out.removed += 1;
                    out.bytes += item.bytes;
                    ("removed", None, item.bytes)
                }
                Err(e) => {
                    out.failed += 1;
                    let msg = e.to_string();
                    out.first_failure
                        .get_or_insert_with(|| format!("{}: {msg}", item.path.display()));
                    ("failed", Some(msg), 0)
                }
            },
        };
        ledger.write(&Line::Item {
            session,
            rule: rule.id.clone(),
            path: item.path.clone(),
            bytes: freed,
            result: result.into(),
            reason,
        });
        if last.elapsed() >= Duration::from_millis(150) {
            progress(&out, total);
            last = Instant::now();
        }
    }
    ledger.write(&Line::Session {
        id: session,
        command: "clean".into(),
        started: session,
        finished: ledger::now(),
        items: out.removed,
        bytes: out.bytes,
        failed: out.failed,
    });
    Ok(out)
}

/// The one place that deletes. Directories go with `remove_dir_all`, which never follows
/// a symlink inside the tree (it unlinks the link).
fn remove(path: &Path, kind: Kind) -> std::io::Result<()> {
    match kind {
        Kind::Dir => std::fs::remove_dir_all(path),
        _ => std::fs::remove_file(path),
    }
}
