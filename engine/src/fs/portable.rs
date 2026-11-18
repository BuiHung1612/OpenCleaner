//! Fallback listing: `read_dir` + `lstat` per entry. Used where the bulk call is missing
//! (network and FAT volumes on macOS, other systems) and as the reference in tests.

use super::{Entry, Kind};
use std::fs::{self, Metadata};
use std::io;
use std::path::Path;

pub fn read_dir(dir: &Path, visit: &mut dyn FnMut(&Entry)) -> io::Result<()> {
    let parent_dev = fs::symlink_metadata(dir).map(|m| dev_of(&m)).ok();
    for item in fs::read_dir(dir)? {
        let item = item?;
        let name = item.file_name();
        let entry = match fs::symlink_metadata(item.path()) {
            Ok(meta) => {
                let mut e = from_metadata(name, &meta);
                // without a mount flag from the system, a directory on another device is one
                e.mount_point = e.kind == Kind::Dir && parent_dev.is_some_and(|d| d != e.dev);
                e
            }
            Err(err) => {
                let mut e = Entry::empty(name);
                e.error = Some(err.raw_os_error().unwrap_or(libc_eio()));
                e
            }
        };
        visit(&entry);
    }
    Ok(())
}

#[cfg_attr(windows, allow(dead_code))] // Windows has its own (with file ids)
pub fn stat(path: &Path) -> io::Result<Entry> {
    let meta = fs::symlink_metadata(path)?;
    let name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    Ok(from_metadata(name, &meta))
}

fn kind_of(meta: &Metadata) -> Kind {
    let t = meta.file_type();
    if t.is_symlink() {
        Kind::Symlink
    } else if t.is_dir() {
        Kind::Dir
    } else if t.is_file() {
        Kind::File
    } else {
        Kind::Other
    }
}

#[cfg(unix)]
fn from_metadata(name: std::ffi::OsString, meta: &Metadata) -> Entry {
    use std::os::unix::fs::MetadataExt;
    Entry {
        kind: kind_of(meta),
        dev: meta.dev(),
        ino: meta.ino(),
        nlink: meta.nlink().min(u32::MAX as u64) as u32,
        alloc: meta.blocks() * 512,
        len: meta.size(),
        mtime: meta.mtime(),
        atime: meta.atime(),
        ..Entry::empty(name)
    }
}

#[cfg(unix)]
fn dev_of(meta: &Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.dev()
}

#[cfg(windows)]
fn from_metadata(name: std::ffi::OsString, meta: &Metadata) -> Entry {
    use std::os::windows::fs::MetadataExt;
    let len = meta.file_size();
    Entry {
        kind: kind_of(meta),
        len,
        // no allocation size here: round up to a 4 KiB cluster, the NTFS default
        alloc: len.div_ceil(4096) * 4096,
        mtime: filetime_to_unix(meta.last_write_time()),
        atime: filetime_to_unix(meta.last_access_time()),
        ..Entry::empty(name)
    }
}

#[cfg(windows)]
fn dev_of(_meta: &Metadata) -> u64 {
    0
}

/// FILETIME (100 ns ticks since 1601) to Unix seconds.
#[cfg(windows)]
pub(crate) fn filetime_to_unix(ticks: u64) -> i64 {
    (ticks / 10_000_000) as i64 - 11_644_473_600
}

#[cfg(not(any(unix, windows)))]
fn from_metadata(name: std::ffi::OsString, meta: &Metadata) -> Entry {
    Entry {
        kind: kind_of(meta),
        len: meta.len(),
        alloc: meta.len(),
        ..Entry::empty(name)
    }
}

#[cfg(not(any(unix, windows)))]
fn dev_of(_meta: &Metadata) -> u64 {
    0
}

fn libc_eio() -> i32 {
    5
}
