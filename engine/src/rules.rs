//! Cleaning rules are data, not code: `rules/<platform>.toml` (embedded at build time)
//! says where junk lives; the engine applies the same guards to all of them.
//!
//! A rule's `paths` are patterns: `~` is the home, `%NAME%` a Windows variable, and a
//! component may hold `*` / `?` wildcards. Each match is a *root*. With `target =
//! "contents"` (the default) every child of a root is one item; with `"self"` the root
//! itself is the item.

use crate::context::Context;
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    #[default]
    Contents,
    #[serde(rename = "self")]
    Itself,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub id: String,
    pub group: String,
    pub title: String,
    pub paths: Vec<String>,
    #[serde(default)]
    pub target: Target,
    /// Only items whose name matches one of these (default: all).
    #[serde(default)]
    pub include: Vec<String>,
    /// Never items whose name matches one of these.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Only items last modified at least this many days ago.
    #[serde(default)]
    pub min_age_days: u32,
    /// Skip the whole rule while one of these apps runs (bundle ids on macOS, exe names on
    /// Windows).
    #[serde(default)]
    pub running: Vec<String>,
    /// Each item is named after an app's bundle id (~/Library/Caches/<id>): skip the items
    /// of apps that are running.
    #[serde(default)]
    pub item_is_bundle_id: bool,
    /// Selected when the review opens. False for things worth a look first.
    #[serde(default = "yes")]
    pub default: bool,
    /// What it is and what cleaning it costs, shown in the review.
    #[serde(default)]
    pub note: String,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleSet {
    #[serde(default, rename = "group")]
    pub groups: Vec<Group>,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

impl RuleSet {
    pub fn parse(text: &str) -> Result<RuleSet, String> {
        let set: RuleSet = toml::from_str(text).map_err(|e| e.to_string())?;
        for r in &set.rules {
            if !set.groups.iter().any(|g| g.id == r.group) {
                return Err(format!("rule {}: unknown group {}", r.id, r.group));
            }
            for p in &r.paths {
                validate_pattern(p).map_err(|e| format!("rule {}: {e}", r.id))?;
            }
        }
        let mut ids: Vec<&str> = set.rules.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        if let Some(w) = ids.windows(2).find(|w| w[0] == w[1]) {
            return Err(format!("duplicate rule id {}", w[0]));
        }
        Ok(set)
    }

    /// The rules shipped for this platform.
    pub fn builtin() -> RuleSet {
        #[cfg(target_os = "macos")]
        let text = include_str!("../rules/macos.toml");
        #[cfg(windows)]
        let text = include_str!("../rules/windows.toml");
        #[cfg(not(any(target_os = "macos", windows)))]
        let text = "";
        RuleSet::parse(text).expect("built-in rules are valid (checked by tests)")
    }
}

/// Patterns must stay where they say: absolute after expansion, no `..`.
fn validate_pattern(p: &str) -> Result<(), String> {
    if !(p.starts_with("~/") || p.starts_with('%')) {
        return Err(format!("{p}: must start with ~/ or %VAR%"));
    }
    if p.split(['/', '\\']).any(|c| c == ".." || c == ".") {
        return Err(format!("{p}: . and .. are not allowed"));
    }
    Ok(())
}

/// `*` (any run) and `?` (one character) within one path component.
pub fn wildcard(pattern: &str, name: &str) -> bool {
    fn go(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => go(&p[1..], n) || (!n.is_empty() && go(p, &n[1..])),
            (Some('?'), Some(_)) => go(&p[1..], &n[1..]),
            (Some(a), Some(b)) if a == b => go(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    go(&p, &n)
}

pub fn matches_any(patterns: &[String], name: &str) -> bool {
    patterns.iter().any(|p| wildcard(p, name))
}

/// Expands a pattern to the existing directories (or files, for `self` rules) it names.
/// Never follows a symlink: a link where a pattern component should be is not a match.
pub fn expand(pattern: &str, ctx: &Context) -> Vec<PathBuf> {
    let (base, rest) = if let Some(rest) = pattern.strip_prefix("~/") {
        (ctx.home.clone(), rest)
    } else if let Some(stripped) = pattern.strip_prefix('%')
        && let Some((name, rest)) = stripped.split_once('%')
    {
        match ctx.vars.get(&name.to_ascii_uppercase()) {
            Some(v) => (v.clone(), rest.trim_start_matches(['/', '\\'])),
            None => return Vec::new(),
        }
    } else {
        return Vec::new();
    };
    let parts: Vec<&str> = rest.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
    let mut found = Vec::new();
    descend(&base, &parts, &mut found);
    found
}

fn descend(dir: &Path, parts: &[&str], found: &mut Vec<PathBuf>) {
    let Some((part, rest)) = parts.split_first() else {
        found.push(dir.to_path_buf());
        return;
    };
    let mut next = Vec::new();
    if part.contains(['*', '?']) {
        let Ok(items) = std::fs::read_dir(dir) else {
            return;
        };
        for item in items.flatten() {
            if let Some(name) = item.file_name().to_str()
                && wildcard(part, name)
            {
                next.push(item.path());
            }
        }
        next.sort();
    } else {
        next.push(dir.join(part));
    }
    for p in next {
        let Ok(meta) = std::fs::symlink_metadata(&p) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if rest.is_empty() || meta.is_dir() {
            descend(&p, rest, found);
        }
    }
}

/// True when `path` is `root` or below it, compared component by component.
pub fn is_within(path: &Path, root: &Path) -> bool {
    let mut p = path.components();
    for r in root.components() {
        if p.next() != Some(r) {
            return false;
        }
    }
    true
}

/// `is_within` for names the file system compares without case (macOS, Windows): each
/// component is compared lowercased. Unicode forms (NFC/NFD) are not normalised here: the
/// exclusion check backs this up by comparing folder identities (dev, ino), which catches
/// them, and the fixed folder names it is used with (PRIVATE…) are ASCII.
pub fn is_within_nocase(path: &Path, root: &Path) -> bool {
    if !cfg!(any(target_os = "macos", windows)) {
        return is_within(path, root);
    }
    let key = |c: Component| c.as_os_str().to_string_lossy().to_lowercase();
    let mut p = path.components();
    for r in root.components() {
        match p.next() {
            Some(c) if key(c) == key(r) => {}
            _ => return false,
        }
    }
    true
}

/// No `.`/`..`, absolute, no control characters.
pub fn is_clean_path(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|c| !matches!(c, Component::CurDir | Component::ParentDir))
        && !path.to_string_lossy().chars().any(char::is_control)
}
