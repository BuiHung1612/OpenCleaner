//! Hidden items in the home folder (`.android`, `.vscode`, `.zshrc.backup.…`): what each
//! belongs to, what it holds (a cache, the tool itself, settings and data), when anything
//! in it last changed, and a verdict: safe to remove, probably outdated, still needed or not
//! recognized. Old backups, files tools rebuild and caches of tools gone or unused start
//! ticked; everything else is only shown. Settings, keys and history are never listed.
//! Everything goes to the Trash.

use crate::context::Context;
use crate::guard::Purpose;
use crate::picks::{Action, Pick};
use crate::size;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::fs::walk::{self, Visitor};
use crate::fs::{self, Detail, Entry, Kind};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::SystemTime;

/// Never listed or removed: settings, keys, credentials, and folders other tools share.
pub const ESSENTIAL: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".config",
    ".local",
    ".cache",
    ".Trash",
    ".password-store",
    ".zsh",
    ".vim",
    ".emacs.d",
    ".tmux",
    ".git",
    ".subversion",
    ".oh-my-zsh",
    ".gcloud",
    ".terraform.d",
    ".cups",
    ".dropbox",
    ".Spotlight-V100",
];

/// Words in names of folders that hold keys and secrets.
const SECRET_WORDS: &[&str] = &[
    "sign", "key", "secret", "token", "credential", "cert", "passw", "wallet", "vault", "auth", "1password", "oci",
];

/// Never listed or removed: in `ESSENTIAL`, or named like a place for keys and secrets.
pub fn is_essential(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    // by word (".app-signing", not ".design")
    let words = || lower.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty());
    ESSENTIAL.iter().any(|e| e.eq_ignore_ascii_case(name))
        || words().any(|w| SECRET_WORDS.iter().any(|s| w.starts_with(s)))
}

/// What the guard lets go: folders, old backups of files that are not essential, and the
/// files zsh rebuilds. Other hidden files are settings, history or tokens.
pub fn removable(name: &str, is_dir: bool) -> bool {
    if is_essential(name) {
        return false;
    }
    match backup_of(name) {
        Some(base) => !is_essential(&base),
        None => is_dir || name.starts_with(".zcompdump"),
    }
}

/// Folders that only hold what their tools download or rebuild: ticked when their tool is
/// gone. Others (`.claude`, `.ollama`, `.android`, `.gradle`…) hold history, models, keys
/// or settings, and are only shown.
const REBUILDABLE: &[&str] = &[
    ".npm", ".yarn", ".cocoapods", ".pub-cache", ".dartServer", ".dart-tool", ".swiftpm", ".konan",
    ".rustup", ".vscode", ".vscode-insiders", ".vscode-oss", ".ServiceHub",
];

/// Who makes a hidden folder: its name, the owner's name, app names (macOS bundle names,
/// the start of Windows' registered names) and commands that show the owner is installed.
/// Owners with neither are the tool itself (`.nvm`): listed by age only.
struct Owner {
    name: &'static str,
    label: &'static str,
    apps: &'static [&'static str],
    commands: &'static [&'static str],
}

const EDITORS: &[&str] = &[
    "Visual Studio Code",
    "Microsoft Visual Studio Code",
    "Cursor",
    "Windsurf",
    "Kiro",
    "Trae",
    "Antigravity",
    "VSCodium",
    "Qoder",
    "IntelliJ IDEA",
    "PyCharm",
    "WebStorm",
    "Android Studio",
];
const VS_CODE: &[&str] = &["Visual Studio Code", "Microsoft Visual Studio Code"];

const fn o(
    name: &'static str,
    label: &'static str,
    apps: &'static [&'static str],
    commands: &'static [&'static str],
) -> Owner {
    Owner {
        name,
        label,
        apps,
        commands,
    }
}

const OWNERS: &[Owner] = &[
    o(
        ".android",
        "Android Studio",
        &["Android Studio"],
        &["adb", "emulator", "flutter"],
    ),
    o(
        ".gradle",
        "Gradle",
        &["Android Studio", "IntelliJ IDEA"],
        &["gradle"],
    ),
    o(
        ".m2",
        "Maven",
        &["IntelliJ IDEA", "Android Studio"],
        &["mvn"],
    ),
    o(".vscode", "Visual Studio Code", VS_CODE, &["code"]),
    o(".vscode-shared", "Visual Studio Code", VS_CODE, &["code"]),
    o(
        ".vscode-react-native",
        "Visual Studio Code",
        VS_CODE,
        &["code"],
    ),
    o(
        ".vscode-insiders",
        "Visual Studio Code Insiders",
        &["Visual Studio Code - Insiders"],
        &["code-insiders"],
    ),
    o(".vscode-oss", "VSCodium", &["VSCodium"], &["codium"]),
    o(".cursor", "Cursor", &["Cursor"], &["cursor"]),
    o(".windsurf", "Windsurf", &["Windsurf"], &["windsurf"]),
    o(".codeium", "Windsurf", &["Windsurf"], &["windsurf"]),
    o(".kiro", "Kiro", &["Kiro"], &["kiro"]),
    o(".trae", "Trae", &["Trae"], &["trae"]),
    o(".trae-cn", "Trae", &["Trae"], &["trae"]),
    o(
        ".antigravity",
        "Antigravity",
        &["Antigravity"],
        &["antigravity"],
    ),
    o(
        ".antigravity-ide",
        "Antigravity",
        &["Antigravity"],
        &["antigravity"],
    ),
    o(".qoder", "Qoder", &["Qoder"], &["qoder"]),
    o(".qodo", "Qodo", EDITORS, &["qodo"]),
    o(".roo", "Roo Code", EDITORS, &[]),
    o(".roorules", "Roo Code", EDITORS, &[]),
    o(".continue", "Continue", EDITORS, &[]),
    o(".supermaven", "Supermaven", EDITORS, &[]),
    o(".specstory", "SpecStory", EDITORS, &[]),
    o(".claude", "Claude Code", &["Claude"], &["claude"]),
    o(".codex", "Codex", &["Codex"], &["codex"]),
    o(".gemini", "Gemini CLI", &[], &["gemini"]),
    o(".cc-switch", "CC Switch", &["CC Switch"], &[]),
    o(".ollama", "Ollama", &["Ollama"], &["ollama"]),
    o(".lmstudio", "LM Studio", &["LM Studio"], &["lms"]),
    o(".orbstack", "OrbStack", &["OrbStack"], &["orb"]),
    o(".colima", "Colima", &[], &["colima"]),
    o(".minikube", "minikube", &[], &["minikube"]),
    o(".vagrant.d", "Vagrant", &[], &["vagrant"]),
    o(".bun", "Bun", &[], &["bun"]),
    o(".deno", "Deno", &[], &["deno"]),
    o(".volta", "Volta", &[], &["volta"]),
    o(".nvm", "nvm", &[], &[]),
    o(".sdkman", "SDKMAN!", &[], &[]),
    o(".npm", "npm", &[], &["npm", "node"]),
    o(".yarn", "Yarn", &[], &["yarn", "node"]),
    o(".expo", "Expo", &[], &["node"]),
    o(".react-native-cli", "React Native", &[], &["node"]),
    o(".storybook", "Storybook", &[], &["node"]),
    o(".supabase", "Supabase CLI", &[], &["supabase"]),
    o(".cargo", "Rust", &[], &["cargo", "rustup"]),
    o(".rustup", "Rust", &[], &["cargo", "rustup"]),
    o(".pub-cache", "Flutter", &[], &["flutter", "dart"]),
    o(".dartServer", "Flutter", &[], &["flutter", "dart"]),
    o(".rbenv", "rbenv", &[], &["rbenv"]),
    o(".pyenv", "pyenv", &[], &["pyenv"]),
    o(".conda", "Conda", &[], &["conda"]),
    o(".ipython", "IPython", &[], &["ipython", "jupyter"]),
    o(".jupyter", "Jupyter", &[], &["jupyter"]),
    o(".gem", "RubyGems", &[], &["gem"]),
    o(".bundle", "Bundler", &[], &["bundle"]),
    o(".cocoapods", "CocoaPods", &[], &["pod"]),
    o(".swiftpm", "Swift", &[], &["swift"]),
    o(".dotnet", ".NET", &[], &["dotnet"]),
    o(".nuget", ".NET", &[], &["dotnet"]),
    o(".aspnet", ".NET", &[], &["dotnet"]),
    o(".templateengine", ".NET", &[], &["dotnet"]),
    o(
        ".ServiceHub",
        "Visual Studio",
        &["Visual Studio", "Microsoft Visual Studio"],
        &["devenv"],
    ),
    o(".shorebird", "Shorebird", &[], &["shorebird"]),
    o(".Genymobile", "Genymotion", &["Genymotion"], &[]),
    o(".maestro", "Maestro", &[], &["maestro"]),
    o(
        ".konan",
        "Kotlin/Native",
        &["Android Studio", "IntelliJ IDEA"],
        &["gradle", "kotlinc"],
    ),
    o(".opencode", "OpenCode", &[], &["opencode"]),
    o(".copilot", "GitHub Copilot CLI", &[], &["copilot"]),
    o(".grok", "Grok CLI", &[], &["grok"]),
    o(".anydesk", "AnyDesk", &["AnyDesk"], &[]),
    o(".postman", "Postman", &["Postman"], &["postman"]),
    o(".orca", "Orca", &["Orca"], &["orca"]),
    o(".matplotlib", "Matplotlib", &[], &["python3"]),
    o(".dart-tool", "Dart", &[], &["dart", "flutter"]),
    o(".dart-cli-completion", "Dart", &[], &["dart", "flutter"]),
    o(".zsh_sessions", "Terminal", &[], &["zsh"]),
];

/// What a known folder holds, for the verdict and its wording.
#[derive(Clone, Copy, PartialEq)]
enum Role {
    /// Downloads and build output its tool fetches or makes again.
    Cache,
    /// The tool itself (toolchains, version managers).
    Tool,
    /// Settings, history, sessions, models, keys.
    Data,
}

const CACHES: &[&str] = &[
    ".npm", ".yarn", ".cocoapods", ".pub-cache", ".dartServer", ".dart-tool", ".dart-cli-completion",
    ".swiftpm", ".konan", ".gradle", ".m2", ".gem", ".nuget", ".matplotlib", ".expo",
    ".react-native-cli", ".ServiceHub", ".templateengine",
];
const TOOLS: &[&str] = &[
    ".nvm", ".sdkman", ".bun", ".deno", ".volta", ".rustup", ".cargo", ".rbenv", ".pyenv", ".conda",
    ".dotnet", ".shorebird", ".maestro", ".bundle",
];

fn role(name: &str) -> Role {
    if CACHES.contains(&name) {
        Role::Cache
    } else if TOOLS.contains(&name) {
        Role::Tool
    } else {
        Role::Data
    }
}

/// Folders inside tools' hidden folders that hold one version per entry: the newest of
/// each kind and the one set as default stay, older ones are listed. (folder, tool, which
/// entries count: a name prefix, "digit" for names starting with a digit, "" for all, and
/// whether old ones may start ticked: not runtimes, whose old versions venvs and projects
/// may point to; downloads like Gradle's or pub's come back when a project needs them)
pub const VERSIONED: &[(&str, &str, &str, bool)] = &[
    (".gradle/wrapper/dists", "Gradle", "", true),
    (".gradle/caches", "Gradle", "digit", true),
    (".gradle/daemon", "Gradle", "digit", true),
    (".nvm/versions/node", "Node.js", "", false),
    (".rbenv/versions", "Ruby", "", false),
    (".pyenv/versions", "Python", "", false),
    (".rustup/toolchains", "Rust", "", true),
    (".konan", "Kotlin/Native", "kotlin-native-prebuilt-", true),
    (".vscode/extensions", "Visual Studio Code", "", true),
    (".vscode-insiders/extensions", "Visual Studio Code Insiders", "", true),
    (".cursor/extensions", "Cursor", "", true),
    (".windsurf/extensions", "Windsurf", "", true),
    (".kiro/extensions", "Kiro", "", true),
    (".trae/extensions", "Trae", "", true),
    (".antigravity/extensions", "Antigravity", "", true),
    (".pub-cache/hosted/pub.dev", "Flutter", "", true),
];

const OLD_VERSIONS: &str = "Old versions";

/// Splits a name at its version: "gradle-8.9-bin" → ("gradle--bin", [8, 9]),
/// "v20.11.0" → ("v", [20, 11, 0]), "ms-python.python-2024.2.0-darwin-arm64" →
/// ("ms-python.python--darwin-arm64", [2024, 2, 0]). The version starts after a "-", or at
/// the start (after an optional "v").
fn split_version(name: &str) -> Option<(String, Vec<u64>, String)> {
    let b = name.as_bytes();
    let start = if b.first().is_some_and(u8::is_ascii_digit) {
        0
    } else if b.first() == Some(&b'v') && b.get(1).is_some_and(u8::is_ascii_digit) {
        1
    } else {
        (1..b.len()).find(|&i| b[i - 1] == b'-' && b[i].is_ascii_digit())?
    };
    let mut end = start;
    while end < b.len() && (b[end].is_ascii_digit() || (b"._-".contains(&b[end]) && b.get(end + 1).is_some_and(u8::is_ascii_digit))) {
        end += 1;
    }
    let numbers = name[start..end].split(['.', '-', '_']).filter_map(|n| n.parse().ok()).collect();
    Some((format!("{}{}", &name[..start], &name[end..]), numbers, name[start..end].to_string()))
}

/// In `defaults`: what the tool uses is unknown, so no version is listed.
const KEEP_ALL: &str = "*";

/// What the tool is set to use (it stays even when a newer one is installed).
fn defaults(home: &Path, folder: &str) -> Vec<String> {
    let read = |p: &str| std::fs::read_to_string(home.join(p)).unwrap_or_default();
    match folder {
        ".rbenv/versions" => read(".rbenv/version").lines().map(|l| l.trim().to_string()).collect(),
        ".pyenv/versions" => read(".pyenv/version").lines().map(|l| l.trim().to_string()).collect(),
        // "20" or "v20.11.0", or another alias ("lts/iron", "lts/*") to follow; one that
        // does not lead to a version keeps them all
        ".nvm/versions/node" => {
            let mut alias = read(".nvm/alias/default").trim().to_string();
            for _ in 0..4 {
                let number = alias.trim_start_matches('v');
                if number.starts_with(|c: char| c.is_ascii_digit()) {
                    return vec![format!("v{number}")];
                }
                if alias.is_empty() || alias.contains("..") {
                    break;
                }
                alias = read(&format!(".nvm/alias/{alias}")).trim().to_string();
            }
            vec![KEEP_ALL.into()]
        }
        ".rustup/toolchains" => read(".rustup/settings.toml")
            .lines()
            .filter_map(|l| l.strip_prefix("default_toolchain"))
            .map(|v| v.trim_start_matches([' ', '=']).trim().trim_matches('"').to_string())
            .collect(),
        _ => vec![],
    }
}

/// Is `rel` (inside the home folder) an entry of a `VERSIONED` folder?
pub fn is_versioned_entry(rel: &Path) -> bool {
    let name = rel.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    rel.parent().is_some_and(|p| {
        VERSIONED.iter().any(|(folder, _, which, _)| p == Path::new(folder) && counts(&name, which))
    }) && split_version(&name).is_some()
}

/// Is `name` one of the entries a `VERSIONED` folder lists?
fn counts(name: &str, which: &str) -> bool {
    !name.starts_with('.')
        && match which {
            "" => true,
            "digit" => name.starts_with(|c: char| c.is_ascii_digit()),
            prefix => name.starts_with(prefix),
        }
}

/// Older versions in `VERSIONED` folders; `place` is the newer version that stays.
fn old_versions(home: &Path) -> Vec<Pick> {
    let mut out = Vec::new();
    for (folder, tool, which, ticked) in VERSIONED {
        let dir = home.join(folder);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let keep = defaults(home, folder);
        if keep.iter().any(|k| k == KEEP_ALL) {
            continue;
        }
        // family → [(numbers, version text, name)]
        let mut families: HashMap<String, Vec<(Vec<u64>, String, String)>> = HashMap::new();
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            if !is_dir || !counts(&name, which) {
                continue;
            }
            if let Some((family, numbers, text)) = split_version(&name) {
                families.entry(family).or_default().push((numbers, text, name));
            }
        }
        for mut versions in families.into_values() {
            versions.sort();
            let Some((_, newest, _)) = versions.pop() else { continue };
            for (_, _, name) in versions {
                if keep.iter().any(|k| name == *k || name.starts_with(&format!("{k}-")) || name.starts_with(&format!("{k}."))) {
                    continue;
                }
                let mut pick = Pick::new(dir.join(&name), home.to_path_buf(), Purpose::HomeItems, Action::Trash);
                pick.flags.push("dot");
                pick.flags.push("inner");
                pick.kind = (*tool).into();
                pick.place = newest.clone();
                if *ticked {
                    pick.flags.push("redownloaded");
                }
                pick.group = format!("{OLD_VERSIONS}: {tool}");
                out.push(pick);
            }
        }
    }
    out
}

/// Verdicts (group names), in the order they are listed.
const SAFE: &str = "Safe to remove";
const OUTDATED: &str = "Probably outdated";
const UNKNOWN: &str = "Not recognized";
const NEEDED: &str = "Still needed";
const ORDER: [&str; 5] = [OLD_VERSIONS, SAFE, OUTDATED, UNKNOWN, NEEDED];

/// Not used for this long: outdated (or a cache safe to clear).
const IDLE_DAYS: i64 = 180;
/// Used this recently: in use, even when its owner is not found.
const RECENT_DAYS: i64 = 30;
/// A removed tool's cache starts ticked once unused this long.
const LEFTOVER_DAYS: i64 = 90;

pub fn home_items(ctx: &Context, cancel: &AtomicBool) -> Vec<Pick> {
    let Ok(entries) = std::fs::read_dir(&ctx.home) else {
        return Vec::new();
    };
    let mut out: Vec<Pick> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !name.starts_with('.') || meta.file_type().is_symlink() || !removable(&name, meta.is_dir()) {
            continue;
        }
        let is_file = meta.is_file();
        // other hidden files are settings, history or tokens
        if is_file && backup_of(&name).is_none() && !name.starts_with(".zcompdump") {
            continue;
        }
        let mut pick = Pick::new(entry.path(), ctx.home.clone(), Purpose::HomeItems, Action::Trash);
        pick.flags.push("dot");
        out.push(pick);
    }
    let first_inner = out.len();
    out.extend(old_versions(&ctx.home));
    // sizes and the last change anywhere inside, in two parallel walks
    let paths: Vec<PathBuf> = out.iter().map(|p| p.path.clone()).collect();
    let (sizes, newest) = std::thread::scope(|s| {
        let newest = s.spawn(|| last_changes(&paths, cancel));
        (size::freed_many(&paths, cancel, &mut |_| {}), newest.join().expect("last changes"))
    });
    let installed = Installed::read(ctx);
    let mut backups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, pick) in out.iter_mut().enumerate() {
        pick.bytes = sizes.get(i).copied().unwrap_or(0);
        let used = newest.get(i).map_or(0, |&t| days_since(t));
        pick.changed_days = Some(used);
        let name = pick.name.clone();
        let mut parts: Vec<String> = Vec::new();
        if i >= first_inner {
            // an older version: the newer one stays; ticked once installed a while ago
            parts.push(pick.kind.clone());
            parts.push(format!("{} is newer", pick.place));
            pick.preselect = used >= LEFTOVER_DAYS && pick.flags.contains(&"redownloaded");
        } else if let Some(base) = backup_of(&name) {
            pick.group = SAFE.into();
            pick.flags.push("backup");
            parts.push(format!("Backup of {base}"));
            pick.kind = base.clone();
            // the live file is there: the backups are spare copies
            if ctx.home.join(&base).exists() {
                backups.entry(base).or_default().push(i);
            }
        } else if name.starts_with(".zcompdump") {
            pick.group = SAFE.into();
            pick.flags.push("rebuilt");
            parts.push("zsh rebuilds it when needed".into());
            pick.preselect = true;
        } else if let Some(owner) = OWNERS.iter().find(|o| o.name.eq_ignore_ascii_case(&name)) {
            pick.kind = owner.label.into();
            let role = role(owner.name);
            let (flag, text) = match role {
                Role::Cache => ("cache", format!("{} cache, downloaded again when needed", owner.label)),
                Role::Tool => ("tool", format!("{} installation", owner.label)),
                Role::Data => ("data", format!("{} settings and data", owner.label)),
            };
            pick.flags.push(flag);
            parts.push(text);
            // something still writes to it, wherever its owner lives
            let present = used < RECENT_DAYS || installed.has(owner, &pick.path);
            if !present {
                pick.flags.push("missing");
                parts.push(format!("{} is not installed", owner.label));
            }
            let rebuildable = REBUILDABLE.contains(&owner.name);
            pick.group = match (role, present) {
                // its tool is gone: caches, extensions and toolchains are of no use now
                (Role::Cache, false) => {
                    pick.preselect = used >= LEFTOVER_DAYS && rebuildable;
                    SAFE
                }
                (_, false) if rebuildable => {
                    pick.preselect = used >= LEFTOVER_DAYS;
                    SAFE
                }
                // its tool is still there: clearing it is the user's call
                (Role::Cache, true) if used >= IDLE_DAYS => SAFE,
                (_, false) => OUTDATED,
                (_, true) if used >= IDLE_DAYS => OUTDATED,
                _ => NEEDED,
            }
            .into();
        } else {
            pick.flags.push("unknown");
            pick.group = if used >= IDLE_DAYS { OUTDATED } else { UNKNOWN }.into();
        }
        // what is inside an installed version does not change after it is installed
        let inner = i >= first_inner;
        parts.push(match used {
            _ if inner && used == 0 => "installed today".into(),
            _ if inner => format!("installed {used} days ago"),
            0 => "last used today".into(),
            1 => "last used yesterday".into(),
            _ => format!("last used {used} days ago"),
        });
        pick.detail = parts.join(" · ");
    }
    // of each file's backups, all but the newest start ticked
    for indexes in backups.values() {
        let newest = indexes.iter().copied().min_by_key(|&i| out[i].changed_days);
        for &i in indexes {
            if Some(i) == newest {
                out[i].flags.push("newest");
                out[i].detail.push_str(" · newest, not ticked");
            } else {
                out[i].preselect = true;
            }
        }
    }
    // old versions first, one group per tool
    out.sort_by(|a, b| {
        let rank = |p: &Pick| ORDER.iter().position(|g| p.group.starts_with(g)).unwrap_or(ORDER.len());
        (rank(a), &a.group, std::cmp::Reverse(a.bytes)).cmp(&(rank(b), &b.group, std::cmp::Reverse(b.bytes)))
    });
    out
}

/// ".zshrc.backup.20260119_175711" → ".zshrc"; ".zprofile.pysave" → ".zprofile".
fn backup_of(name: &str) -> Option<String> {
    if let Some(base) = name.strip_suffix('~') {
        return (base.len() > 1).then(|| base.into());
    }
    let lower = name.to_ascii_lowercase();
    // a marker at the end, or before a stamp: ".bak.1787286373", ".backup-2", not ".original"
    [".backup", ".bak", ".pysave", ".orig", ".old", ".save"]
        .iter()
        .filter_map(|m| {
            let at = lower[1..].find(m)? + 1;
            let rest = &lower[at + m.len()..];
            (rest.is_empty() || rest.starts_with(['.', '-', '_']) || rest.starts_with(|c: char| c.is_ascii_digit()))
                .then_some(at)
        })
        .min()
        .filter(|&at| at > 1)
        .map(|at| name[..at].to_string())
}

/// The newest modification time of each item and everything inside it (one parallel walk).
fn last_changes(items: &[PathBuf], cancel: &AtomicBool) -> Vec<SystemTime> {
    struct Newest(Vec<AtomicI64>);
    impl Visitor for Newest {
        fn entry(&self, _dir: &Path, tag: u32, e: &Entry) -> Option<u32> {
            self.0[tag as usize].fetch_max(e.mtime, Ordering::Relaxed);
            (e.kind == Kind::Dir && !e.mount_point).then_some(tag)
        }
        fn error(&self, _path: &Path, _tag: u32, _err: std::io::Error) {}
    }
    let newest = Newest(items.iter().map(|_| AtomicI64::new(0)).collect());
    let mut roots = Vec::new();
    for (i, path) in items.iter().enumerate() {
        if let Ok(e) = fs::stat(path, Detail::Basic) {
            newest.0[i].store(e.mtime, Ordering::Relaxed);
            if e.kind == Kind::Dir {
                roots.push((path.clone(), i as u32));
            }
        }
    }
    walk::walk_tagged(roots, walk::default_threads(), Detail::Basic, cancel, &newest);
    newest
        .0
        .iter()
        .map(|t| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(t.load(Ordering::Relaxed).max(0) as u64))
        .collect()
}

fn days_since(t: SystemTime) -> i64 {
    SystemTime::now()
        .duration_since(t)
        .map_or(0, |d| (d.as_secs() / 86_400) as i64)
}

/// Installed apps and the folders commands are found in, read once per scan.
struct Installed {
    apps: Vec<String>,
    dirs: Vec<PathBuf>,
}

impl Installed {
    fn read(ctx: &Context) -> Installed {
        let running = crate::procs::Running::snapshot();
        // by name and by bundle file name (Visual Studio Code calls itself "Code")
        let apps = crate::apps::list(ctx, &running)
            .into_iter()
            .flat_map(|a| {
                [
                    a.name.to_lowercase(),
                    a.path
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_lowercase())
                        .unwrap_or_default(),
                ]
            })
            .filter(|n| !n.is_empty())
            .collect();
        // an app started from the Finder has a short PATH: also where installers put commands
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from));
        for d in [
            ".local/bin",
            ".cargo/bin",
            ".bun/bin",
            ".volta/bin",
            ".deno/bin",
            ".pub-cache/bin",
            "flutter/bin",
        ] {
            dirs.push(ctx.home.join(d));
        }
        if let Some(appdata) = ctx.vars.get("APPDATA") {
            dirs.push(appdata.join("npm"));
        }
        // node from nvm
        if let Ok(versions) = std::fs::read_dir(ctx.home.join(".nvm/versions/node")) {
            dirs.extend(versions.flatten().map(|v| v.path().join("bin")));
        }
        Installed { apps, dirs }
    }

    fn has(&self, owner: &Owner, folder: &Path) -> bool {
        if owner.apps.is_empty() && owner.commands.is_empty() {
            return true;
        }
        let app = owner.apps.iter().any(|a| {
            let a = a.to_lowercase();
            self.apps.iter().any(|n| n.starts_with(&a))
        });
        app || owner.commands.iter().any(|c| {
            self.dirs.iter().chain([&folder.join("bin")]).any(|d| {
                d.join(c).is_file()
                    || (cfg!(windows)
                        && ["exe", "cmd"]
                            .iter()
                            .any(|x| d.join(format!("{c}.{x}")).is_file()))
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backups() {
        assert_eq!(
            backup_of(".zshrc.backup.20260119_175711").as_deref(),
            Some(".zshrc")
        );
        assert_eq!(backup_of(".zprofile.pysave").as_deref(), Some(".zprofile"));
        assert_eq!(backup_of(".vimrc~").as_deref(), Some(".vimrc"));
        assert_eq!(backup_of(".backup"), None);
        assert_eq!(backup_of(".android"), None);
        assert_eq!(backup_of(".config.original"), None);
        assert_eq!(backup_of(".zshrc.bak.1787286373").as_deref(), Some(".zshrc"));
        assert!(!removable(".ssh.bak", true) && !removable(".zshrc", false) && !removable(".netrc", false));
        assert!(removable(".zshrc.bak", false) && removable(".zcompdump", false) && removable(".gradle", true));
        assert!(is_essential(".app-signing") && !is_essential(".design"));
    }

    #[test]
    fn versions() {
        let v = |n: &str| split_version(n).map(|(f, v, _)| (f, v));
        assert_eq!(v("gradle-8.9-bin"), Some(("gradle--bin".into(), vec![8, 9])));
        assert_eq!(v("v20.11.0"), Some(("v".into(), vec![20, 11, 0])));
        assert_eq!(v("9.4.1"), Some(("".into(), vec![9, 4, 1])));
        assert_eq!(v("ms-python.python-2024.2.0-darwin-arm64"), Some(("ms-python.python--darwin-arm64".into(), vec![2024, 2, 0])));
        assert_eq!(v("stable-aarch64-apple-darwin"), None);
        assert_eq!(v("nightly-2024-01-05-aarch64-apple-darwin").map(|x| x.1), Some(vec![2024, 1, 5]));
    }

    #[test]
    fn nvm_default_follows_aliases() {
        let home = std::env::temp_dir().join(format!("oc-nvm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".nvm/alias/lts")).unwrap();
        // "lts/*" is a file name only Unix allows; the chain is the same
        std::fs::write(home.join(".nvm/alias/default"), "lts/latest\n").unwrap();
        std::fs::write(home.join(".nvm/alias/lts/latest"), "lts/jod").unwrap();
        std::fs::write(home.join(".nvm/alias/lts/jod"), "v22.11.0").unwrap();
        assert_eq!(defaults(&home, ".nvm/versions/node"), ["v22.11.0"]);
        std::fs::write(home.join(".nvm/alias/default"), "node").unwrap();
        assert_eq!(defaults(&home, ".nvm/versions/node"), [KEEP_ALL]);
        std::fs::write(home.join(".nvm/alias/default"), "20").unwrap();
        assert_eq!(defaults(&home, ".nvm/versions/node"), ["v20"]);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn old_versions_inside_folders() {
        let home = std::env::temp_dir().join(format!("oc-ver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        for d in [".gradle/wrapper/dists/gradle-8.9-bin", ".gradle/wrapper/dists/gradle-9.1-bin", ".gradle/wrapper/dists/gradle-8.9-all",
                  ".rbenv/versions/3.1.0", ".rbenv/versions/3.2.0", ".rbenv/versions/3.3.0", ".gradle/caches/modules-2"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        std::fs::write(home.join(".rbenv/version"), "3.1.0\n").unwrap();
        let old: Vec<String> = old_versions(&home).iter().map(|p| p.name.clone()).collect();
        // 8.9-all is the only "all"; 3.1.0 is the default; modules-2 is not a version
        assert_eq!(old.len(), 2, "{old:?}");
        assert!(old.contains(&"gradle-8.9-bin".to_string()) && old.contains(&"3.2.0".to_string()));
        let ctx = Context { home: home.clone(), vars: HashMap::new(), data_dir: home.join("data") };
        let ok = |rel: &str| crate::guard::check_for(&home.join(rel), &home, &ctx, Purpose::HomeItems).is_ok();
        assert!(ok(".gradle/wrapper/dists/gradle-8.9-bin") && ok(".rbenv/versions/3.2.0"));
        assert!(!ok(".gradle/caches/modules-2") && !ok(".gradle/caches/modules-2/x") && !ok(".rbenv/version"));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn lists_home_items() {
        let home = std::env::temp_dir().join(format!("oc-dot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join(".mystery")).unwrap();
        std::fs::create_dir_all(home.join(".app-signing")).unwrap();
        std::fs::create_dir_all(home.join(".ssh.bak")).unwrap();
        std::fs::create_dir_all(home.join(".ollama")).unwrap();
        for f in [
            ".zshrc",
            ".zshrc.backup.1",
            ".zshrc.backup.2",
            ".zcompdump-host",
            ".bash_history",
        ] {
            std::fs::write(home.join(f), "x").unwrap();
        }
        let ctx = Context {
            home: home.clone(),
            vars: HashMap::new(),
            data_dir: home.join("data"),
        };
        let items = home_items(&ctx, &AtomicBool::new(false));
        let names: Vec<&str> = items.iter().map(|p| p.name.as_str()).collect();
        assert!(
            !names.contains(&".ssh")
                && !names.contains(&".zshrc")
                && !names.contains(&".bash_history")
        );
        assert!(names.contains(&".mystery") && names.contains(&".zcompdump-host"));
        assert!(items.iter().all(|p| p.name != ".ssh.bak"));
        // the last check before removal, whatever the list said
        use crate::guard::check_for;
        let check = |n: &str| check_for(&home.join(n), &home, &ctx, Purpose::HomeItems).is_ok();
        assert!(check(".mystery") && check(".zshrc.backup.1") && check(".zcompdump-host"));
        assert!(!check(".ssh") && !check(".ssh.bak") && !check(".zshrc") && !check(".bash_history"));
        assert!(!check(".mystery/inside") && !check("visible"));
        // an app's folder that may hold keys or history is never ticked, even when left over
        #[cfg(unix)]
        {
            let old = SystemTime::now() - std::time::Duration::from_secs(400 * 86_400);
            std::fs::File::open(home.join(".ollama")).unwrap().set_modified(old).unwrap();
            let again = home_items(&ctx, &AtomicBool::new(false));
            assert!(again.iter().any(|p| p.name == ".ollama" && !p.preselect));
        }
        let backups: Vec<&Pick> = items.iter().filter(|p| p.flags.contains(&"backup")).collect();
        assert_eq!(backups.len(), 2);
        assert_eq!(backups.iter().filter(|p| p.preselect).count(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }
}
