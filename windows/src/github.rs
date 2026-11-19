//! GitHub without a service: tell the user when a newer release exists (no self-replacement:
//! the exe is unsigned, so there is nothing to verify a download against), and after a crash
//! offer a prefilled issue built from the log the panic hook wrote.

#![cfg_attr(not(windows), allow(dead_code))]

const REPO: &str = "BuiHung1612/OpenCleaner";
const VERSION: &str = include_str!("../../VERSION");

/// "1.0.10" is newer than "1.0.9".
fn is_newer(remote: &str, local: &str) -> bool {
    let parse = |v: &str| v.trim().trim_start_matches('v').split('.').map(|n| n.parse().unwrap_or(0)).collect::<Vec<u64>>();
    let (r, l) = (parse(remote), parse(local));
    (0..r.len().max(l.len())).map(|i| (r.get(i).unwrap_or(&0), l.get(i).unwrap_or(&0))).find(|(a, b)| a != b).is_some_and(|(a, b)| a > b)
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// New-issue link with the log's first 40 lines, fewer if the URL would pass `limit` characters.
fn issue_url(version: &str, os: &str, log: &str, limit: usize) -> String {
    let base = format!("https://github.com/{REPO}/issues/new?title={}&body=", encode(&format!("Crash: OpenCleaner {version} (Windows)")));
    let mut lines: Vec<&str> = log.lines().take(40).collect();
    loop {
        let body = format!("OpenCleaner {version}, {os}\n\nCrash log (first {} lines):\n```\n{}\n```\n", lines.len(), lines.join("\n"));
        let url = base.clone() + &encode(&body);
        if url.len() <= limit || lines.pop().is_none() {
            return url;
        }
    }
}

#[cfg(windows)]
mod win {
    use crate::ui::wide;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn crash_log() -> Option<PathBuf> {
        Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("OpenCleaner").join("crash.log"))
    }

    /// Output of a console tool, without flashing a console window.
    fn quiet(program: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(program).args(args).creation_flags(CREATE_NO_WINDOW).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn ask(text: &str) -> bool {
        let (text, caption) = (wide(text), wide("OpenCleaner"));
        unsafe { MessageBoxW(null_mut(), text.as_ptr(), caption.as_ptr(), MB_YESNO | MB_ICONINFORMATION | MB_SETFOREGROUND) == IDYES }
    }

    fn open(url: &str) {
        let (verb, url) = (wide("open"), wide(url));
        unsafe { ShellExecuteW(null_mut(), verb.as_ptr(), url.as_ptr(), null(), null(), SW_SHOWNORMAL) };
    }

    /// Writes the panic message and backtrace to %LOCALAPPDATA%\OpenCleaner\crash.log.
    pub fn install_panic_hook() {
        std::panic::set_hook(Box::new(|info| {
            let Some(path) = crash_log() else { return };
            let _ = std::fs::create_dir_all(path.parent().unwrap());
            let text = format!("{info}\n\n{}", std::backtrace::Backtrace::force_capture());
            let _ = std::fs::write(path, text);
        }));
    }

    /// Runs off the UI thread: the crash report offer, then the update check.
    pub fn on_launch() {
        let version = super::VERSION.trim();
        if let Some(path) = crash_log()
            && let Ok(log) = std::fs::read_to_string(&path)
        {
            let _ = std::fs::remove_file(&path); // ask once per crash
            if ask("OpenCleaner quit unexpectedly last time.\n\nReport it on GitHub? You can review the crash details before posting.") {
                let os = quiet("cmd", &["/c", "ver"]).map(|v| v.trim().to_string()).unwrap_or_else(|| "Windows".into());
                open(&super::issue_url(version, &os, &log, 7000));
            }
        }
        // curl.exe ships with Windows 10 1803 and later; offline or older systems just stay quiet
        let api = format!("https://api.github.com/repos/{}/releases/latest", super::REPO);
        let Some(reply) = quiet("curl.exe", &["-fsSL", "-m", "15", "-H", "User-Agent: OpenCleaner", &api]) else { return };
        let tag = serde_json::from_str::<serde_json::Value>(&reply).ok().and_then(|v| v["tag_name"].as_str().map(str::to_owned));
        if let Some(tag) = tag
            && super::is_newer(&tag, version)
            && ask(&format!("A newer version is available: OpenCleaner {}. You have {version}.\n\nOpen the download page?", tag.trim_start_matches('v')))
        {
            open(&format!("https://github.com/{}/releases/latest", super::REPO));
        }
    }
}

#[cfg(windows)]
pub use win::{install_panic_hook, on_launch};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.1.1", "0.1.0"));
        assert!(is_newer("1.0.10", "1.0.9\n"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("0.9.9", "1.0.0"));
    }

    #[test]
    fn issue_url_is_capped() {
        let log: String = (1..=100).map(|i| format!("line {i} {}\n", "x".repeat(200))).collect();
        let url = issue_url("1.0", "Windows 11", &log, 7000);
        assert!(url.len() <= 7000 && url.contains("line%2010%20") && !url.contains("line%2041%20"));
        assert!(url.starts_with("https://github.com/BuiHung1612/OpenCleaner/issues/new?title=Crash%3A%20OpenCleaner%201.0"));
    }
}
