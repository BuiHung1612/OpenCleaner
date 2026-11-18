//! File system access. Each platform lists a directory in bulk (one system call returns
//! many entries with their sizes), so a walk costs one call per batch of entries instead
//! of one `lstat` per file:
//!
//! - macOS: `getattrlistbulk(2)` (`mac.rs`)
//! - Windows: `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)` (`win.rs`)
//! - anything else, and volumes without the bulk call: `read_dir` + `lstat` (`portable.rs`)

use std::ffi::OsString;
use std::io;
use std::path::Path;

#[cfg(target_os = "macos")]
mod mac;
mod portable;
pub mod walk;
#[cfg(windows)]
mod win;

/// How much to ask the system for each entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// Type, ids, link count, allocated and logical size, times, flags.
    Basic,
    /// Basic plus each file's private size (`Entry::private`). On APFS this makes the
    /// kernel look up every file's extents and costs about twice the walk time, so only
    /// "what would deleting free" asks for it.
    Freed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One directory entry, as much as the platform reports in its bulk listing.
#[derive(Clone, Debug)]
pub struct Entry {
    pub name: OsString,
    pub kind: Kind,
    /// Volume the entry lives on. For a mount point this is the volume underneath:
    /// check `mount_point`.
    pub dev: u64,
    /// File id (inode). 0 when unknown.
    pub ino: u64,
    /// Hard link count; 1 when unknown.
    pub nlink: u32,
    /// Bytes allocated on disk (what `du` counts).
    pub alloc: u64,
    /// Logical length.
    pub len: u64,
    /// Bytes only this file holds, i.e. what deleting it frees (APFS clones share blocks).
    pub private: Option<u64>,
    /// Transparently compressed (its private size is not reliable).
    pub compressed: bool,
    /// Modification and access times, seconds since the Unix epoch.
    pub mtime: i64,
    pub atime: i64,
    /// A directory where another volume is mounted.
    pub mount_point: bool,
    /// The entry is listed but its attributes could not be read.
    pub error: Option<i32>,
}

impl Entry {
    fn empty(name: OsString) -> Self {
        Entry {
            name,
            kind: Kind::Other,
            dev: 0,
            ino: 0,
            nlink: 1,
            alloc: 0,
            len: 0,
            private: None,
            compressed: false,
            mtime: 0,
            atime: 0,
            mount_point: false,
            error: None,
        }
    }

    /// Bytes deleting this file would free: its private size when the volume knows it
    /// and the file is not compressed, never more than what it occupies.
    pub fn freed(&self) -> u64 {
        match self.private {
            Some(p) if !self.compressed => p.min(self.alloc),
            _ => self.alloc,
        }
    }
}

/// Calls `visit` for every entry of the directory `dir` (not recursive, no `.`/`..`).
pub fn read_dir(dir: &Path, detail: Detail, visit: &mut dyn FnMut(&Entry)) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        match mac::read_dir(dir, detail, visit) {
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {}
            other => return other,
        }
    }
    #[cfg(windows)]
    {
        let _ = detail;
        match win::read_dir(dir, visit) {
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {}
            other => return other,
        }
    }
    let _ = detail;
    portable::read_dir(dir, visit)
}

/// The `read_dir` + `lstat` listing, for comparing against the bulk one in tests.
#[doc(hidden)]
pub fn read_dir_portable(dir: &Path, visit: &mut dyn FnMut(&Entry)) -> io::Result<()> {
    portable::read_dir(dir, visit)
}

/// The entry for `path` itself, without following a final symlink.
pub fn stat(path: &Path, detail: Detail) -> io::Result<Entry> {
    #[cfg(target_os = "macos")]
    {
        match mac::stat(path, detail) {
            Err(e) if e.kind() == io::ErrorKind::Unsupported => {}
            other => return other,
        }
    }
    #[cfg(windows)]
    {
        let _ = detail;
        win::stat(path)
    }
    #[cfg(not(windows))]
    {
        let _ = detail;
        portable::stat(path)
    }
}
