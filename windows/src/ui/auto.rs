//! Scheduled System Junk cleaning while the app is open (as on macOS, Automation.swift):
//! checked at launch and every hour, weekly or monthly, only the rules selected by default.
//! No scheduled task: nothing runs when the app is closed. The outcome shows as a tray balloon.

use serde_json::Value;
use windows_sys::Win32::Foundation::HWND;

pub const ENABLED_VALUE: &str = "AutoClean";
pub const MONTHLY_VALUE: &str = "AutoCleanMonthly";
/// Unix seconds of the last run (a DWORD lasts until 2106)
pub const LAST_VALUE: &str = "AutoCleanLast";

const DAY: u64 = 86_400;

/// Whether a run is due. A clock set back before the last run counts as due, so it cannot
/// stall the schedule.
// ponytail: a month is 30 days, not a calendar month; fine for a cleaning schedule
pub fn is_due(last: Option<u64>, now: u64, monthly: bool) -> bool {
    elapsed(last, now, if monthly { 30 * DAY } else { 7 * DAY })
}

fn elapsed(last: Option<u64>, now: u64, period: u64) -> bool {
    last.is_none_or(|last| now < last || now >= last + period)
}

// Alerts (Settings > Automation), off by default: free space on the system drive, and the
// Recycle Bin and Downloads growing. Each one at most once a day.
pub const DISK_VALUE: &str = "AlertDisk";
pub const DISK_GB_VALUE: &str = "AlertDiskGB";
pub const DISK_CHOICES: [u32; 4] = [5, 10, 20, 50];
pub const WATCH_VALUE: &str = "AlertWatch";
pub const WATCH_GB_VALUE: &str = "AlertWatchGB";
pub const WATCH_CHOICES: [u32; 4] = [1, 5, 10, 20];

/// Checks the alerts that are on, on a thread (the readings take seconds), and shows what
/// is due as one tray balloon.
pub fn check_alerts(hwnd: HWND) {
    use super::{SETTINGS_KEY, pages::bytes, reg_dword, set_reg_dword};
    let on = |name| reg_dword(SETTINGS_KEY, name) == Some(1);
    let (disk, watch) = (on(DISK_VALUE), on(WATCH_VALUE));
    if !disk && !watch {
        return;
    }
    let target = hwnd as isize;
    std::thread::spawn(move || {
        let gb = |name, choices: [u32; 4], default| reg_dword(SETTINGS_KEY, name).filter(|v| choices.contains(v)).unwrap_or(default) as f64 * 1e9;
        let now = now();
        // not shown in the last day; stamped once shown
        let due = |name: &str| elapsed(reg_dword(SETTINGS_KEY, name).map(u64::from), now, DAY);
        let mut lines: Vec<(&str, String)> = Vec::new();
        // read directly: a status reading would also move the Dashboard's CPU baseline
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        if disk
            && due("AlertDiskLast")
            && let Some(free) = free_bytes(&format!("{system}\\"))
            && free < gb(DISK_GB_VALUE, DISK_CHOICES, 10)
        {
            lines.push(("AlertDiskLast", format!("Drive {system} is almost full: {} free", bytes(Some(free)))));
        }
        let limit = gb(WATCH_GB_VALUE, WATCH_CHOICES, 5);
        if watch && due("AlertBinLast") && let Some(bin) = recycle_bin_bytes().filter(|&b| b > limit) {
            lines.push(("AlertBinLast", format!("The Recycle Bin holds {}", bytes(Some(bin)))));
        }
        if watch && due("AlertDownloadsLast") {
            let downloads = oc_engine::context::Context::from_env().user_folder("Downloads");
            let reply = super::jobs::call(serde_json::json!({"op": "size", "paths": [downloads]}));
            if let Some(size) = reply["events"][0]["report"]["bytes"].as_f64().filter(|&b| b > limit) {
                lines.push(("AlertDownloadsLast", format!("Downloads holds {}", bytes(Some(size)))));
            }
        }
        for (name, _) in &lines {
            set_reg_dword(SETTINGS_KEY, name, now as u32);
        }
        let text: Vec<&str> = lines.iter().map(|(_, l)| l.as_str()).collect();
        match text[..] {
            [] => {}
            [one] => notify(target as HWND, one, "Open OpenCleaner to clean up."),
            _ => notify(target as HWND, "Time to clean up", &text.join(". ")),
        }
    });
}

/// Free bytes for the current user on the drive of `root` ("C:\\").
fn free_bytes(root: &str) -> Option<f64> {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
    let mut free = 0u64;
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then_some(free as f64)
}

/// What the Recycle Bins of all drives hold.
fn recycle_bin_bytes() -> Option<f64> {
    use windows_sys::Win32::UI::Shell::{SHQUERYRBINFO, SHQueryRecycleBinW};
    let mut info: SHQUERYRBINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<SHQUERYRBINFO>() as u32;
    (unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut info) } == 0).then_some(info.i64Size.max(0) as f64)
}

/// The rule ids an unattended clean may touch: selected by default, something to remove,
/// never the Recycle Bin, never privacy (history, cookies).
pub fn safe_rules(plan: &Value) -> Vec<String> {
    plan["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|g| g["rules"].as_array().into_iter().flatten())
        .filter(|r| r["default"] == true && r["status"] == "ready")
        .filter(|r| !["trash", "privacy"].contains(&r["group"].as_str().unwrap_or("")) && r["id"] != "trash")
        .filter_map(|r| r["id"].as_str().map(String::from))
        .collect()
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// A balloon from the notification area (a toast on Windows 10 and 11). The icon is added on
/// first use and removed when the window closes (`remove_tray`).
pub fn notify(hwnd: HWND, title: &str, text: &str) {
    use windows_sys::Win32::UI::Shell::*;
    let mut nid = tray(hwnd);
    nid.uFlags = NIF_ICON | NIF_TIP | NIF_INFO;
    nid.hIcon = unsafe {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        windows_sys::Win32::UI::WindowsAndMessaging::LoadIconW(GetModuleHandleW(std::ptr::null()), std::ptr::without_provenance::<u16>(1))
    };
    nid.dwInfoFlags = NIIF_INFO;
    copy(&mut nid.szTip, "OpenCleaner");
    copy(&mut nid.szInfoTitle, title);
    copy(&mut nid.szInfo, text);
    unsafe {
        if Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
            Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }
}

pub fn remove_tray(hwnd: HWND) {
    use windows_sys::Win32::UI::Shell::{NIM_DELETE, Shell_NotifyIconW};
    unsafe { Shell_NotifyIconW(NIM_DELETE, &tray(hwnd)) };
}

fn tray(hwnd: HWND) -> windows_sys::Win32::UI::Shell::NOTIFYICONDATAW {
    let mut nid: windows_sys::Win32::UI::Shell::NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of_val(&nid) as u32;
    nid.hWnd = hwnd;
    nid.uID = 1;
    nid
}

/// `s` into a fixed UTF-16 field, cut to fit, NUL-terminated.
fn copy(field: &mut [u16], s: &str) {
    let n = field.len() - 1;
    for (dst, src) in field.iter_mut().zip(s.encode_utf16().take(n).chain([0])) {
        *dst = src;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn due() {
        let t = 1_800_000_000;
        assert!(is_due(None, t, false), "never ran");
        assert!(!is_due(Some(t), t + 6 * DAY, false));
        assert!(is_due(Some(t), t + 7 * DAY, false));
        assert!(!is_due(Some(t), t + 29 * DAY, true));
        assert!(is_due(Some(t), t + 30 * DAY, true));
        assert!(is_due(Some(t), t - DAY, false), "clock set back");
        assert!(!elapsed(Some(t), t + DAY - 1, DAY), "an alert at most once a day");
        assert!(elapsed(Some(t), t + DAY, DAY));
    }

    #[test]
    fn only_safe_rules() {
        let rule = |id: &str, group: &str, default: bool, status: &str| json!({"id": id, "group": group, "default": default, "status": status});
        let plan = json!({"groups": [
            {"id": "system", "rules": [rule("temp", "system", true, "ready"), rule("logs", "system", false, "ready"), rule("crash", "system", true, "in_use"), rule("empty", "system", true, "empty")]},
            {"id": "privacy", "rules": [rule("privacy.chrome.cookies", "privacy", true, "ready")]},
            {"id": "trash", "rules": [rule("trash", "trash", true, "ready")]},
        ]});
        assert_eq!(safe_rules(&plan), vec!["temp".to_string()]);
    }
}
