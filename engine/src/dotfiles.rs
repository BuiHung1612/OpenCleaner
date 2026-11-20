//! Hidden items in the home folder (`.android`, `.vscode`, `.zshrc.backup.…`): what each
//! belongs to and whether it still earns its place. Old backups and files tools rebuild
//! start ticked; folders of apps that are gone are ticked once unchanged for 90 days;
//! everything else is only shown. Settings, keys and history are never listed.
//! Everything goes to the Trash.

use crate::context::Context;
use crate::guard::Purpose;
use crate::picks::{Action, Pick};
use crate::size;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
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
    ".npm", ".yarn", ".cocoapods", ".pub-cache", ".dartServer", ".swiftpm", ".konan", ".vscode",
    ".vscode-shared", ".vscode-react-native", ".vscode-insiders", ".vscode-oss", ".cursor",
    ".windsurf", ".kiro", ".trae", ".trae-cn", ".antigravity", ".antigravity-ide", ".qoder",
    ".bun", ".deno", ".rustup", ".ServiceHub", ".dart-tool", ".expo", ".react-native-cli",
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

/// Group names, in the order they are listed.
const BACKUPS: &str = "Old backups";
const REBUILT: &str = "Rebuilt automatically";
const LEFTOVERS: &str = "Left by apps no longer installed";
const IDLE: &str = "Not changed for 6 months";
const IN_USE: &str = "In use";
const UNKNOWN: &str = "Not recognized";
const ORDER: [&str; 6] = [BACKUPS, REBUILT, LEFTOVERS, IDLE, UNKNOWN, IN_USE];

const IDLE_DAYS: i64 = 180;
/// Changed this recently: in use, even when its owner is not found.
const RECENT_DAYS: i64 = 30;
/// A missing app's folder starts ticked once unchanged this long.
const LEFTOVER_DAYS: i64 = 90;

pub fn home_items(ctx: &Context, cancel: &AtomicBool) -> Vec<Pick> {
    let Ok(entries) = std::fs::read_dir(&ctx.home) else {
        return Vec::new();
    };
    let installed = Installed::read(ctx);
    let mut out: Vec<Pick> = Vec::new();
    let mut backups: HashMap<String, Vec<usize>> = HashMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !name.starts_with('.') || name == "." || name == ".." || meta.file_type().is_symlink() {
            continue;
        }
        if is_essential(&name) {
            continue;
        }
        let path = entry.path();
        let changed = days_since(newest_change(&path, &meta));
        let mut pick = Pick::new(
            path.clone(),
            ctx.home.clone(),
            Purpose::HomeItems,
            Action::Trash,
        );
        pick.changed_days = Some(changed);
        pick.flags.push("dot");
        if !removable(&name, meta.is_dir()) {
            continue;
        }
        if let Some(base) = backup_of(&name) {
            pick.group = BACKUPS.into();
            pick.flags.push("backup");
            pick.detail = format!("Backup of {base}");
            pick.kind = base.clone();
            // the live file is there: the backups are spare copies
            if ctx.home.join(&base).exists() {
                backups.entry(base).or_default().push(out.len());
            }
        } else if name.starts_with(".zcompdump") && meta.is_file() {
            pick.group = REBUILT.into();
            pick.flags.push("rebuilt");
            pick.detail = "zsh rebuilds it when needed".into();
            pick.preselect = true;
        } else if meta.is_file() {
            // settings, history, tokens: small and personal
            continue;
        } else if let Some(owner) = OWNERS.iter().find(|o| o.name.eq_ignore_ascii_case(&name)) {
            pick.kind = owner.label.into();
            // something still writes to it, wherever its owner lives
            if changed < RECENT_DAYS || installed.has(owner, &path) {
                if changed >= IDLE_DAYS {
                    pick.group = IDLE.into();
                    pick.flags.push("idle");
                    pick.detail = format!("{} · unchanged for {changed} days", owner.label);
                } else {
                    pick.group = IN_USE.into();
                    pick.flags.push("in_use");
                    pick.detail = owner.label.into();
                }
            } else {
                pick.group = LEFTOVERS.into();
                pick.flags.push("missing");
                pick.detail = format!(
                    "{} is not installed · unchanged for {changed} days",
                    owner.label
                );
                pick.preselect = changed >= LEFTOVER_DAYS && REBUILDABLE.contains(&owner.name);
            }
        } else {
            pick.group = UNKNOWN.into();
            pick.flags.push("unknown");
            pick.detail = format!("Unchanged for {changed} days");
        }
        out.push(pick);
    }
    // of each file's backups, all but the newest start ticked
    for indexes in backups.values() {
        let newest = indexes
            .iter()
            .copied()
            .max_by_key(|&i| std::cmp::Reverse(out[i].changed_days));
        for &i in indexes {
            if Some(i) == newest {
                out[i].flags.push("newest");
                out[i].detail.push_str(" · newest, not ticked");
            } else {
                out[i].preselect = true;
            }
        }
    }
    let paths: Vec<PathBuf> = out.iter().map(|p| p.path.clone()).collect();
    for (pick, bytes) in out
        .iter_mut()
        .zip(size::freed_many(&paths, cancel, &mut |_| {}))
    {
        pick.bytes = bytes;
    }
    out.sort_by_key(|p| {
        (
            ORDER
                .iter()
                .position(|g| *g == p.group)
                .unwrap_or(ORDER.len()),
            std::cmp::Reverse(p.bytes),
        )
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

/// The newest change of the item and what it directly holds.
// ponytail: one level deep, a walk of the whole tree if folders read as idle while in use
fn newest_change(path: &Path, meta: &std::fs::Metadata) -> SystemTime {
    let mut newest = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if meta.is_dir()
        && let Ok(children) = std::fs::read_dir(path)
    {
        for child in children.flatten() {
            if let Ok(t) = child.metadata().and_then(|m| m.modified()) {
                newest = newest.max(t);
            }
        }
    }
    newest
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
        // an app's folder that may hold keys or history is never ticked, even when left over
        #[cfg(unix)]
        {
            let old = SystemTime::now() - std::time::Duration::from_secs(400 * 86_400);
            std::fs::File::open(home.join(".ollama")).unwrap().set_modified(old).unwrap();
            let again = home_items(&ctx, &AtomicBool::new(false));
            assert!(again.iter().any(|p| p.name == ".ollama" && !p.preselect));
        }
        let backups: Vec<&Pick> = items.iter().filter(|p| p.group == BACKUPS).collect();
        assert_eq!(backups.len(), 2);
        assert_eq!(backups.iter().filter(|p| p.preselect).count(), 1);
        let _ = std::fs::remove_dir_all(&home);
    }
}
