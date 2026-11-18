//! OpenCleaner engine: scanning, sizing and cleaning for macOS and Windows.
//!
//! The app links this crate as a static library and talks to it through the C ABI in
//! `ffi.rs` (JSON requests and events); `oce` is the same engine on the command line.

pub mod api;
pub mod ffi;
pub mod fs;
pub mod size;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
