//! History: every removal attempt and every session, as JSON Lines in
//! `<data dir>/history.jsonl`, so what was removed can always be looked up.

use crate::context::Context;
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
    },
    /// A startup item turned off, with what it takes to turn it back on: the plist or
    /// shortcut's original path (it went to the Trash), or a Run value's name and command.
    Startup {
        session: u64,
        path: PathBuf,
        label: String,
        program: String,
    },
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
        if let (Some(f), Ok(text)) = (self.file.as_mut(), serde_json::to_string(line)) {
            let _ = writeln!(f, "{text}");
        }
    }
}

/// Sessions newest first, in the shape the History page reads.
pub fn sessions(ctx: &Context, limit: usize) -> Value {
    let mut out = Vec::new();
    if let Ok(f) = fs::File::open(path(ctx)) {
        for line in BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(Line::Session {
                command,
                started,
                items,
                bytes,
                failed,
                ..
            }) = serde_json::from_str(&line)
            {
                out.push(json!({
                    "command": command,
                    "started_unix": started,
                    "items": items,
                    "bytes": bytes,
                    "failed": failed,
                }));
            }
        }
    }
    out.reverse();
    out.truncate(limit);
    json!({"sessions": out, "logs": {"deletions": path(ctx)}})
}

pub fn exists(ctx: &Context) -> bool {
    Path::new(&path(ctx)).exists()
}
