//! History: every removal attempt and every session, as JSON Lines in
//! `<data dir>/history.jsonl`, so what was removed can always be looked up.

use crate::context::Context;
use crate::guard::{self, Stamp};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn path(ctx: &Context) -> PathBuf {
    ctx.data_dir.join("history.jsonl")
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Line {
    Session {
        id: u64,
        command: String,
        started: u64,
        finished: u64,
        items: u64,
        bytes: u64,
        failed: u64,
    },
    Item {
        session: u64,
        rule: String,
        path: PathBuf,
        bytes: u64,
        result: String,
        reason: Option<String>,
        /// Where it went in the Trash, for putting it back (absent in older lines).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trashed: Option<PathBuf>,
        /// The item in the Trash right after it went there: only that object is put back
        /// (absent in older lines, which are then not put back).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stamp: Option<Stamp>,
    },
    /// A startup item turned off, with what it takes to turn it back on: the plist or
    /// shortcut's original path (it went to the Trash), or a Run value's name and command.
    Startup {
        session: u64,
        path: PathBuf,
        label: String,
        program: String,
        /// Where the plist went in the Trash (absent in older lines and for Run values).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trashed: Option<PathBuf>,
        /// The plist as it was in the Trash right after it went there: turning it back on
        /// moves only that very file (absent in older lines and for Run values).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stamp: Option<crate::guard::Stamp>,
        /// A Run value's registry type (REG_SZ 1, REG_EXPAND_SZ 2), restored as it was
        /// (absent in older lines and for files).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reg_type: Option<u32>,
    },
}

impl Line {
    /// A session that did nothing (all skipped): not worth a row in the History.
    fn is_empty_session(&self) -> bool {
        matches!(
            self,
            Line::Session {
                items: 0,
                bytes: 0,
                failed: 0,
                ..
            }
        )
    }
}

/// Appends lines; history is best effort and never stops a cleanup.
pub struct Writer {
    file: Option<fs::File>,
}

impl Writer {
    pub fn open(ctx: &Context) -> Writer {
        let _ = fs::create_dir_all(&ctx.data_dir);
        Writer {
            file: OpenOptions::new()
                .create(true)
                .append(true)
                .open(path(ctx))
                .ok(),
        }
    }

    pub fn write(&mut self, line: &Line) {
        if line.is_empty_session() {
            return;
        }
        if let (Some(f), Ok(text)) = (self.file.as_mut(), serde_json::to_string(line)) {
            let _ = writeln!(f, "{text}");
        }
    }
}

/// Skipped and failed items listed per session in History.
const PROBLEMS_SHOWN: usize = 30;

/// Sessions newest first, in the shape the History page reads. `trashed`: what the
/// session moved to the Trash and is still there, so it can be put back. `problems`: the
/// first items it skipped or could not remove, each with why.
pub fn sessions(ctx: &Context, limit: usize) -> Value {
    let mut out = Vec::new();
    // items come before their session line
    let mut trashed: Vec<Value> = Vec::new();
    let mut problems: Vec<Value> = Vec::new();
    let mut freed = Vec::new();
    for line in lines(ctx) {
        match line {
            Line::Item {
                path,
                trashed: Some(to),
                stamp: Some(was),
                ..
            } if guard::stamp(&to) == Some(was) => {
                trashed.push(json!({"from": path, "to": to}));
            }
            Line::Item {
                path,
                result,
                reason,
                ..
            } if result == "failed" || result == "skipped" => {
                if problems.len() < PROBLEMS_SHOWN {
                    problems.push(json!({"path": path, "result": result, "reason": reason}));
                }
            }
            Line::Item { .. } | Line::Startup { .. } => {}
            // written before empty sessions were left out
            l if l.is_empty_session() => {}
            Line::Session {
                command,
                started,
                items,
                bytes,
                failed,
                ..
            } => {
                freed.push((started, bytes));
                out.push(json!({
                "command": command,
                "started_unix": started,
                "items": items,
                "bytes": bytes,
                "failed": failed,
                "trashed": std::mem::take(&mut trashed),
                "problems": std::mem::take(&mut problems),
                }))
            }
        }
    }
    out.reverse();
    out.truncate(limit);
    json!({"sessions": out, "months": months(&freed), "logs": {"deletions": path(ctx)}})
}

/// Bytes freed and sessions per month (UTC), oldest first, from (started, bytes) pairs.
pub fn months(sessions: &[(u64, u64)]) -> Vec<Value> {
    let mut by: std::collections::BTreeMap<String, (u64, u64)> = Default::default();
    for &(started, bytes) in sessions {
        let m = by.entry(month(started)).or_default();
        m.0 += bytes;
        m.1 += 1;
    }
    by.into_iter()
        .map(|(month, (bytes, sessions))| json!({"month": month, "bytes": bytes, "sessions": sessions}))
        .collect()
}

/// "YYYY-MM" of a Unix time, in UTC (days to civil date, Howard Hinnant's algorithm).
// ponytail: UTC, so a cleanup late on the last evening of a month can count for the next
// one in time zones east of UTC; use the local offset if that ever matters.
pub fn month(unix: u64) -> String {
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}")
}

pub(crate) fn lines(ctx: &Context) -> impl Iterator<Item = Line> {
    fs::File::open(path(ctx))
        .into_iter()
        .flat_map(|f| BufReader::new(f).lines().map_while(Result::ok))
        .filter_map(|l| serde_json::from_str(&l).ok())
}

/// What OpenCleaner recorded when it moved `from` to `to` in the Trash (the latest such
/// line). Only those are put back, and only when the record says what the item was.
pub fn trashed_stamp(ctx: &Context, from: &Path, to: &Path) -> Result<Stamp, &'static str> {
    let mut found = None;
    for l in lines(ctx) {
        if let Line::Item {
            path,
            trashed: Some(t),
            stamp,
            ..
        } = l
            && path == from
            && t == to
        {
            found = Some(stamp);
        }
    }
    match found {
        None => Err("not moved to the Trash by OpenCleaner"),
        Some(None) => {
            Err("moved before OpenCleaner recorded what it moved; put it back from the Trash")
        }
        Some(Some(s)) => Ok(s),
    }
}

pub fn exists(ctx: &Context) -> bool {
    Path::new(&path(ctx)).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn months_add_up_per_calendar_month() {
        assert_eq!(month(0), "1970-01");
        assert_eq!(month(951_782_400), "2000-02"); // 2000-02-29 00:00 UTC
        assert_eq!(month(1_798_761_599), "2026-12"); // 2026-12-31 23:59:59
        assert_eq!(month(1_798_761_600), "2027-01");
        let m = months(&[(1_798_761_599, 10), (1_798_761_000, 5), (1_798_761_600, 7)]);
        assert_eq!(m.len(), 2);
        assert_eq!(
            m[0],
            json!({"month": "2026-12", "bytes": 15, "sessions": 2})
        );
        assert_eq!(m[1], json!({"month": "2027-01", "bytes": 7, "sessions": 1}));
    }

    #[test]
    fn sessions_that_did_nothing_are_left_out() {
        let dir = std::env::temp_dir().join(format!("oc-ledger-{}", std::process::id()));
        let ctx = Context {
            home: dir.clone(),
            vars: Default::default(),
            data_dir: dir.clone(),
        };
        let session = |items, bytes, failed| Line::Session {
            id: 1,
            command: "slim".into(),
            started: 1,
            finished: 1,
            items,
            bytes,
            failed,
        };
        let mut w = Writer::open(&ctx);
        w.write(&session(0, 0, 0));
        w.write(&session(0, 0, 1));
        w.write(&session(2, 10, 0));
        drop(w);
        // an empty one from before the rule
        let mut f = OpenOptions::new().append(true).open(path(&ctx)).unwrap();
        writeln!(f, "{}", serde_json::to_string(&session(0, 0, 0)).unwrap()).unwrap();
        let s = sessions(&ctx, 10);
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(s["sessions"].as_array().unwrap().len(), 2);
    }
}
