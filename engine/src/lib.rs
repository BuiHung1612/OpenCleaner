//! OpenCleaner engine: scanning, sizing and cleaning for macOS and Windows.
//!
//! The app links this crate as a static library and talks to it through the C ABI in
//! `ffi.rs` (JSON requests and events); `oce` is the same engine on the command line.

pub mod api;
pub mod apps;
pub mod clean;
pub mod context;
pub mod devtools;
pub mod dircache;
pub mod dotfiles;
pub mod ffi;
pub mod files;
pub mod finders;
pub mod fs;
#[cfg(target_os = "macos")]
pub mod fsevents;
pub mod guard;
pub mod hogs;
pub mod ledger;
pub mod maintenance;
pub mod picks;
pub mod procs;
pub mod rules;
pub mod size;
#[cfg(target_os = "macos")]
pub mod slim;
pub mod space;
pub mod startup;
pub mod status;
pub mod trash;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
