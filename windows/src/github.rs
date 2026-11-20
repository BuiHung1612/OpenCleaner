//! GitHub without a service: install a newer release (checked against the SHA-256 GitHub lists
//! for the asset), and after a crash offer a prefilled issue built from the log the panic hook wrote.

#![cfg_attr(not(windows), allow(dead_code))]

const REPO: &str = "BuiHung1612/OpenCleaner";
pub const VERSION: &str = include_str!("../../VERSION");

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

/// The hash from `certutil -hashfile <file> SHA256` output (second line; older versions
/// put spaces between the bytes), lowercase.
fn certutil_hash(out: &str) -> Option<String> {
    let hex: String = out.lines().nth(1)?.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
    (hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit())).then_some(hex)
}

#[cfg(windows)]
mod win {
    use crate::ui::wide;
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::Command;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, S_OK, WPARAM};
    use windows_sys::Win32::UI::Controls::*;
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
        let (text, caption) = (wide(&crate::i18n::tr(text)), wide("OpenCleaner"));
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
        // what the last update replaced (a running exe can be renamed, not deleted)
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_file(exe.with_extension("old.exe"));
        }
        if crate::ui::reg_dword(crate::ui::SETTINGS_KEY, super::CHECK_VALUE) != Some(0) {
            check(false);
        }
    }

    /// Downloads the new exe next to this one, checks it against the SHA-256 GitHub lists
    /// for the release asset, swaps it in (the running exe is renamed aside) and starts it.
    /// Returns only on failure, with what went wrong (empty when the user cancelled); this exe
    /// is then left as it was.
    fn install(url: &str, digest: &str, size: u64, version: &str) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let new = exe.with_extension("new.exe");
        let old = exe.with_extension("old.exe");
        let new_s = new.to_string_lossy().into_owned();
        download(url, &new, size, version)?;
        let hash = quiet("certutil", &["-hashfile", &new_s, "SHA256"]).and_then(|o| super::certutil_hash(&o));
        if hash.as_deref() != Some(digest.trim_start_matches("sha256:").to_ascii_lowercase().as_str()) {
            let _ = std::fs::remove_file(&new);
            return Err("The download did not match the release (SHA-256 differs), so it was not installed.".into());
        }
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&exe, &old).map_err(|e| format!("Could not replace OpenCleaner: {e}"))?;
        if let Err(e) = std::fs::rename(&new, &exe) {
            let _ = std::fs::rename(&old, &exe);
            return Err(format!("Could not replace OpenCleaner: {e}"));
        }
        Command::new(&exe).spawn().map_err(|e| format!("Installed, but could not start it: {e}"))?;
        // the new version is running: this one ends here (asked first, so nothing is mid-way)
        std::process::exit(0);
    }

    struct Download {
        child: std::process::Child,
        file: PathBuf,
        size: u64,
        done: Option<bool>,
        cancelled: bool,
    }

    fn megabytes(n: u64) -> String {
        format!("{:.1} MB", n as f64 / 1e6)
    }

    /// Progress dialog: polls the file curl is writing, closes itself when curl exits.
    unsafe extern "system" fn on_progress(hwnd: HWND, msg: i32, _: WPARAM, _: LPARAM, data: isize) -> i32 {
        let d = unsafe { &mut *(data as *mut Download) };
        match msg {
            TDN_TIMER => {
                let got = std::fs::metadata(&d.file).map(|m| m.len()).unwrap_or(0);
                let text = wide(&format!("{} of {}", megabytes(got), megabytes(d.size)));
                unsafe {
                    SendMessageW(hwnd, TDM_SET_PROGRESS_BAR_POS as u32, (got * 100 / d.size.max(1)).min(100) as usize, 0);
                    SendMessageW(hwnd, TDM_UPDATE_ELEMENT_TEXT as u32, TDE_CONTENT as usize, text.as_ptr() as isize);
                }
                if let Ok(Some(status)) = d.child.try_wait() {
                    d.done = Some(status.success());
                    unsafe { SendMessageW(hwnd, TDM_CLICK_BUTTON as u32, IDCANCEL as usize, 0) };
                }
            }
            TDN_BUTTON_CLICKED if d.done.is_none() => {
                d.cancelled = true;
                let _ = d.child.kill();
            }
            _ => {}
        }
        S_OK
    }

    /// curl.exe into `file`, with a progress dialog the user can cancel.
    fn download(url: &str, file: &std::path::Path, size: u64, version: &str) -> Result<(), String> {
        let failed = "The download failed. Check your internet connection, or that this folder can be written to.";
        let child = Command::new("curl.exe")
            .args(["-fsSL", "-m", "600", "-H", "User-Agent: OpenCleaner", "-o"])
            .arg(file)
            .arg(url)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|_| failed)?;
        let mut d = Download { child, file: file.to_owned(), size, done: None, cancelled: false };
        let title = wide("OpenCleaner");
        let main = wide(&crate::i18n::tr(&format!("Downloading OpenCleaner {version}")));
        let content = wide(&format!("{} of {}", megabytes(0), megabytes(size)));
        let mut cfg: TASKDIALOGCONFIG = unsafe { std::mem::zeroed() };
        cfg.cbSize = size_of::<TASKDIALOGCONFIG>() as u32;
        cfg.dwFlags = TDF_SHOW_PROGRESS_BAR | TDF_CALLBACK_TIMER;
        cfg.dwCommonButtons = TDCBF_CANCEL_BUTTON;
        cfg.pszWindowTitle = title.as_ptr();
        cfg.pszMainInstruction = main.as_ptr();
        cfg.pszContent = content.as_ptr();
        cfg.pfCallback = Some(on_progress);
        cfg.lpCallbackData = &mut d as *mut Download as isize;
        unsafe { TaskDialogIndirect(&cfg, null_mut(), null_mut(), null_mut()) };
        // the dialog could not open, or closed some other way: the download still decides
        let ok = d.done.unwrap_or_else(|| !d.cancelled && d.child.wait().is_ok_and(|s| s.success()));
        if d.cancelled {
            let _ = d.child.wait();
        }
        if ok && !d.cancelled {
            return Ok(());
        }
        let _ = std::fs::remove_file(file);
        Err(if d.cancelled { String::new() } else { failed.into() })
    }

    /// Asks GitHub for the latest release. `manual` (Settings > Check Now) also says when
    /// there is nothing newer, or when GitHub could not be reached.
    pub fn check(manual: bool) {
        let version = super::VERSION.trim();
        // curl.exe ships with Windows 10 1803 and later; offline or older systems just stay quiet
        let api = format!("https://api.github.com/repos/{}/releases/latest", super::REPO);
        let reply = quiet("curl.exe", &["-fsSL", "-m", "15", "-H", "User-Agent: OpenCleaner", &api]);
        let release = reply.and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok());
        let tag = release.as_ref().and_then(|v| v["tag_name"].as_str().map(str::to_owned));
        let page = format!("https://github.com/{}/releases/latest", super::REPO);
        match tag {
            Some(tag) if super::is_newer(&tag, version) => {
                let new = tag.trim_start_matches('v');
                let asset = release.as_ref().and_then(|r| r["assets"].as_array()?.iter().find(|a| a["name"] == "OpenCleaner.exe").cloned());
                let found = asset.as_ref().and_then(|a| Some((a["browser_download_url"].as_str()?.to_owned(), a["digest"].as_str()?.to_owned(), a["size"].as_u64()?)));
                match found {
                    Some((url, digest, size)) => {
                        if ask(&format!("OpenCleaner {new} is available. You have {version}.\n\nInstall it now? OpenCleaner restarts when it is done."))
                            && let Err(e) = install(&url, &digest, size, new)
                            && !e.is_empty()
                            && ask(&format!("{e}\n\nOpen the download page to install it yourself?"))
                        {
                            open(&page);
                        }
                    }
                    None => {
                        if ask(&format!("OpenCleaner {new} is available. You have {version}.\n\nOpen the download page?")) {
                            open(&page);
                        }
                    }
                }
            }
            Some(_) if manual => tell(&format!("OpenCleaner {version} is the latest version.")),
            None if manual => tell("Could not reach GitHub to check for updates. Check your internet connection and try again."),
            _ => {}
        }
    }

    fn tell(text: &str) {
        let (text, caption) = (wide(&crate::i18n::tr(text)), wide("OpenCleaner"));
        unsafe { MessageBoxW(null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND) };
    }
}

/// Settings: look for a newer release at launch (absent: on).
pub const CHECK_VALUE: &str = "CheckUpdates";

#[cfg(windows)]
pub use win::{check, install_panic_hook, on_launch};

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
    fn certutil_output() {
        let h = "8f7f66daaafe883874fe0f1273dcf70bd2a518fa865a14e493dfb6e66e759559";
        let out = format!("SHA256 hash of C:\\x.exe:\r\n{h}\r\nCertUtil: -hashfile command completed successfully.\r\n");
        assert_eq!(certutil_hash(&out).as_deref(), Some(h));
        let spaced = format!("SHA256 hash of file x:\n{}\nCertUtil: ok\n", h.as_bytes().chunks(2).map(|c| std::str::from_utf8(c).unwrap().to_uppercase()).collect::<Vec<_>>().join(" "));
        assert_eq!(certutil_hash(&spaced).as_deref(), Some(h));
        assert_eq!(certutil_hash("CertUtil: error\n"), None);
    }

    #[test]
    fn issue_url_is_capped() {
        let log: String = (1..=100).map(|i| format!("line {i} {}\n", "x".repeat(200))).collect();
        let url = issue_url("1.0", "Windows 11", &log, 7000);
        assert!(url.len() <= 7000 && url.contains("line%2010%20") && !url.contains("line%2041%20"));
        assert!(url.starts_with("https://github.com/BuiHung1612/OpenCleaner/issues/new?title=Crash%3A%20OpenCleaner%201.0"));
    }
}
