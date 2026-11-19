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
    let period = if monthly { 30 * DAY } else { 7 * DAY };
    last.is_none_or(|last| now < last || now >= last + period)
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
