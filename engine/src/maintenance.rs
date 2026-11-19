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
            admin: false,
            commands: &[&["/usr/bin/qlmanage", "-r", "cache"]],
        },
        Task {
            id: "fonts",
            title: "Clear font caches",
            note: "Fixes garbled or missing fonts. Takes effect after you log out.",
            admin: false,
            commands: &[&["/usr/bin/atsutil", "databases", "-removeUser"]],
        },
        Task {
            id: "launchservices",
            title: "Rebuild the \u{201c}Open With\u{201d} list",
            note: "Removes duplicate or deleted apps from Open With menus.",
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
        .flat_map(|t| {
            t.commands.iter().map(|cmd| {
                cmd.iter()
                    .map(|a| shell_quote(a))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        })
        .collect();
    (!lines.is_empty()).then(|| lines.join(" ; "))
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
pub fn touchid_script(enable: bool) -> String {
    let current = std::fs::read_to_string(SUDO_LOCAL).unwrap_or_default();
    let content = touchid_content(&current, enable);
    format!(
        "/usr/bin/printf '%s' {content} > {file} && /bin/chmod 444 {file} && /usr/sbin/chown root:wheel {file}",
        content = shell_quote(&content),
        file = SUDO_LOCAL
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let t = touchid_script(true);
        assert!(t.contains("/etc/pam.d/sudo_local") && t.contains("pam_tid.so"));
    }
}
