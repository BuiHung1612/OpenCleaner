//! macOS maintenance: Optimization tasks and Touch ID for sudo.
//!
//! The engine never asks for a password. Tasks that need administrator rights come back
//! as one fixed shell script (built only from constants in this file and the current
//! content of a root-owned system file), which the app runs once through the system's
//! authorisation dialog.

use serde::Serialize;
use std::process::Command;

#[derive(Clone, Debug, Serialize)]
pub struct Task {
    pub id: &'static str,
    pub title: &'static str,
    pub note: &'static str,
    pub tradeoff: &'static str,
    /// Needs the administrator password (run by the app, not the engine).
    pub admin: bool,
    #[serde(skip)]
    commands: &'static [&'static [&'static str]],
}

/// Safe, repeatable maintenance. Each only clears caches the system rebuilds.
pub fn tasks() -> Vec<Task> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    vec![
        Task {
            id: "dns",
            title: "Flush the DNS cache",
            note: "Fixes sites that moved or fail to load after a network change.",
            tradeoff: "The next lookup may take longer while DNS records are fetched again. This does not improve your connection speed or remove browser history.",
            admin: true,
            commands: &[
                &["/usr/bin/dscacheutil", "-flushcache"],
                &["/usr/bin/killall", "-HUP", "mDNSResponder"],
            ],
        },
        Task {
            id: "quicklook",
            title: "Reset Quick Look thumbnails",
            note: "Fixes wrong or missing file previews; thumbnails are made again as you browse.",
            tradeoff: "Previews may appear slowly and use extra CPU while thumbnails rebuild. Your original files stay intact.",
            admin: false,
            commands: &[&["/usr/bin/qlmanage", "-r", "cache"]],
        },
        Task {
            id: "fonts",
            title: "Clear font caches",
            note: "Fixes garbled or missing fonts. Takes effect after you log out.",
            tradeoff: "Save your work and log out yourself after running this. Font caches rebuild next time you sign in; this does not install or remove fonts.",
            admin: false,
            commands: &[&["/usr/bin/atsutil", "databases", "-removeUser"]],
        },
        Task {
            id: "launchservices",
            title: "Rebuild the \u{201c}Open With\u{201d} list",
            note: "Removes duplicate or deleted apps from Open With menus.",
            tradeoff: "Rebuilds app registrations for all installed apps; Open With menus may take time to repopulate. Review your preferred default apps afterwards.",
            admin: false,
            commands: &[&[
                "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
                "-kill",
                "-r",
                "-domain",
                "local",
                "-domain",
                "system",
                "-domain",
                "user",
            ]],
        },
        Task {
            id: "memory",
            title: "Free up memory",
            note: "macOS manages memory well on its own. This drops its file caches (purge), so apps may be slower right after while they reload. Useful mostly just before a heavy task.",
            tradeoff: "The gain is temporary: caches fill again, and apps may pause or read more from disk. This does not fix a memory leak or add RAM; skip it when the Mac is working normally.",
            admin: true,
            commands: &[&["/usr/sbin/purge"]],
        },
    ]
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskResult {
    pub id: String,
    pub ok: bool,
    pub message: Option<String>,
}

/// Runs the chosen tasks that need no password; the others are left for `admin_script`.
pub fn run_user(ids: &[String]) -> Vec<TaskResult> {
    tasks()
        .into_iter()
        .filter(|t| !t.admin && ids.iter().any(|i| i == t.id))
        .map(|t| {
            let mut message = None;
            let ok =
                t.commands
                    .iter()
                    .all(|cmd| match Command::new(cmd[0]).args(&cmd[1..]).output() {
                        Ok(out) if out.status.success() => true,
                        Ok(out) => {
                            message = Some(String::from_utf8_lossy(&out.stderr).trim().to_string());
                            false
                        }
                        Err(e) => {
                            message = Some(e.to_string());
                            false
                        }
                    });
            TaskResult {
                id: t.id.into(),
                ok,
                message,
            }
        })
        .collect()
}

/// One script for the chosen tasks that need the administrator password, or None.
pub fn admin_script(ids: &[String]) -> Option<String> {
    let lines: Vec<String> = tasks()
        .into_iter()
        .filter(|t| t.admin && ids.iter().any(|i| i == t.id))
        .map(|t| admin_task_script(&t))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn admin_task_script(task: &Task) -> String {
    let commands = task
        .commands
        .iter()
        .map(|cmd| {
            cmd.iter()
                .map(|a| shell_quote(a))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join(" && ");
    // Stop on the first failed command within a task, but still run and report other tasks.
    format!(
        "oc_output=$( ( {commands} ) 2>&1 )\noc_result=$?\n/usr/bin/printf '\\nOPEN_CLEANER_TASK_RESULT\\t%s\\t%s\\n%s\\n' {} \"$oc_result\" \"$oc_output\"",
        shell_quote(task.id)
    )
}

/// Single-quoted for /bin/sh.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// --- Touch ID for sudo ------------------------------------------------------------------

/// sudo reads this file (macOS 14+) and system updates leave it alone.
pub const SUDO_LOCAL: &str = "/etc/pam.d/sudo_local";
const TID_LINE: &str = "auth       sufficient     pam_tid.so";

#[derive(Clone, Debug, Serialize)]
pub struct TouchId {
    /// The Mac supports sudo_local (macOS 14 or later).
    pub supported: bool,
    pub enabled: bool,
    pub file: &'static str,
}

fn is_tid_line(line: &str) -> bool {
    let l = line.trim();
    !l.starts_with('#')
        && l.split_whitespace().collect::<Vec<_>>() == ["auth", "sufficient", "pam_tid.so"]
}

pub fn touchid_status() -> TouchId {
    let supported = cfg!(target_os = "macos")
        && std::path::Path::new("/etc/pam.d/sudo").exists()
        && sudo_includes_local();
    let enabled = std::fs::read_to_string(SUDO_LOCAL).is_ok_and(|s| s.lines().any(is_tid_line));
    TouchId {
        supported,
        enabled,
        file: SUDO_LOCAL,
    }
}

/// /etc/pam.d/sudo pulls in sudo_local (true on macOS 14+).
fn sudo_includes_local() -> bool {
    std::fs::read_to_string("/etc/pam.d/sudo").is_ok_and(|s| {
        s.lines()
            .any(|l| l.contains("sudo_local") && !l.trim_start().starts_with('#'))
    })
}

/// The new content of sudo_local with Touch ID on or off; every other line is kept.
pub fn touchid_content(current: &str, enable: bool) -> String {
    let mut lines: Vec<&str> = current.lines().filter(|l| !is_tid_line(l)).collect();
    if enable {
        // Touch ID goes first so it is tried before the password
        let at = lines
            .iter()
            .position(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
            .unwrap_or(lines.len());
        lines.insert(at, TID_LINE);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The script that writes sudo_local, root-owned and read-only like the system's own.
pub fn touchid_script(enable: bool) -> Result<String, String> {
    touchid_script_for(std::path::Path::new(SUDO_LOCAL), enable)
}

/// Only a missing file counts as empty: one that cannot be read is never overwritten
/// (its other lines would be lost).
fn touchid_script_for(file: &std::path::Path, enable: bool) -> Result<String, String> {
    let current = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(format!(
                "{} cannot be read ({e}); not changed",
                file.display()
            ));
        }
    };
    let content = touchid_content(&current, enable);
    let file = shell_quote(&file.to_string_lossy());
    Ok(format!(
        "/usr/bin/printf '%s' {content} > {file} && /bin/chmod 444 {file} && /usr/sbin/chown root:wheel {file}",
        content = shell_quote(&content),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn admin_tasks_report_failures_independently() {
        let failed = Task {
            id: "failed",
            title: "",
            note: "",
            tradeoff: "",
            admin: true,
            commands: &[&["false"], &["true"]],
        };
        let passed = Task {
            id: "passed",
            commands: &[&["true"]],
            ..failed.clone()
        };
        let script = format!(
            "{}\n{}",
            admin_task_script(&failed),
            admin_task_script(&passed)
        );
        // Only shell builtins: never execute real maintenance or request administrator rights.
        let result = Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(result.status.success());
        let output = String::from_utf8(result.stdout).unwrap();
        assert!(output.contains("OPEN_CLEANER_TASK_RESULT\tfailed\t1"));
        assert!(output.contains("OPEN_CLEANER_TASK_RESULT\tpassed\t0"));
    }

    #[test]
    fn touchid_keeps_other_lines_and_toggles_only_its_own() {
        let current = "# sudo_local: local config file which survives system update\n\
                       auth       optional       /opt/homebrew/lib/pam/pam_reattach.so\n";
        let on = touchid_content(current, true);
        assert!(on.lines().any(is_tid_line));
        assert!(on.contains("pam_reattach.so"));
        // pam_tid comes before other auth lines
        let tid = on.lines().position(is_tid_line).unwrap();
        let reattach = on.lines().position(|l| l.contains("pam_reattach")).unwrap();
        assert!(tid < reattach);
        // idempotent
        assert_eq!(touchid_content(&on, true), on);
        let off = touchid_content(&on, false);
        assert!(!off.lines().any(is_tid_line));
        assert!(off.contains("pam_reattach.so") && off.contains("# sudo_local"));
        // a commented-out line is not "enabled" and is kept
        assert!(!is_tid_line("#auth sufficient pam_tid.so"));
    }

    #[test]
    fn scripts_are_built_from_quoted_constants() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        let ids = vec!["dns".to_string(), "quicklook".to_string()];
        let script = admin_script(&ids).unwrap_or_default();
        if cfg!(target_os = "macos") {
            assert!(script.contains("'/usr/bin/dscacheutil' '-flushcache'"));
            assert!(
                !script.contains("qlmanage"),
                "user tasks are not in the admin script"
            );
        }
        assert!(admin_script(&["nope".into()]).is_none());
        let dir = std::env::temp_dir().join(format!("oc-tid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // missing: written from scratch
        let t = touchid_script_for(&dir.join("sudo_local"), true).unwrap();
        assert!(t.contains("sudo_local") && t.contains("pam_tid.so"));
        // there but unreadable (here: a folder): refused, never replaced by one line
        assert!(touchid_script_for(&dir, true).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
