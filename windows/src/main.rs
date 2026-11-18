//! OpenCleaner for Windows: the same pages as the macOS app, painted natively (Win32 +
//! GDI+), on the same Rust engine, called directly (no C ABI needed here).

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() {
    ui::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("OpenCleaner for Windows runs on Windows; on macOS use the app in macos/.");
}
