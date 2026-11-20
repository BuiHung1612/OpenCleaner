//! OpenCleaner for Windows: the same pages as the macOS app, painted natively (Win32 +
//! GDI+), on the same Rust engine, called directly (no C ABI needed here).

#![cfg_attr(all(windows, not(test)), windows_subsystem = "windows")]

mod github;
mod i18n;
#[cfg(windows)]
mod ui;

#[cfg(windows)]
fn main() {
    github::install_panic_hook();
    std::thread::spawn(github::on_launch);
    ui::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("OpenCleaner for Windows runs on Windows; on macOS use the app in macos/.");
}
