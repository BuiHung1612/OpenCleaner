//! Windows listing with `GetFileInformationByHandleEx(FileIdBothDirectoryInfo)`: one call
//! fills a buffer with many entries, each with its allocation size, length, file id,
//! attributes and times (what `FindFirstFile` gives, plus the size on disk).

use super::portable::filetime_to_unix;
use super::{Entry, Kind};
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, ERROR_NOT_SUPPORTED, GetLastError,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_ID_BOTH_DIR_INFO,
    FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo, GetFileInformationByHandle,
    GetFileInformationByHandleEx, OPEN_EXISTING,
};

const BUF_LEN: usize = 64 * 1024;

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// `\\?\`-prefixed, NUL-terminated wide path, so long paths work.
pub(crate) fn wide(path: &Path) -> Vec<u16> {
    let raw: Vec<u16> = path.as_os_str().encode_wide().collect();
    let mut out = Vec::with_capacity(raw.len() + 5);
    let verbatim = raw.starts_with(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16]);
    let unc = raw.starts_with(&[b'\\' as u16, b'\\' as u16]);
    if !verbatim && path.is_absolute() {
        if unc {
            out.extend("\\\\?\\UNC".encode_utf16());
            out.extend_from_slice(&raw[1..]);
        } else {
            out.extend("\\\\?\\".encode_utf16());
            out.extend_from_slice(&raw);
        }
    } else {
        out.extend_from_slice(&raw);
    }
    out.push(0);
    out
}

pub fn read_dir(dir: &Path, visit: &mut dyn FnMut(&Entry)) -> io::Result<()> {
    let path = wide(dir);
    let h = unsafe {
        CreateFileW(
            path.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let h = Handle(h);

    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let volume = if unsafe { GetFileInformationByHandle(h.0, &mut info) } != 0 {
        info.dwVolumeSerialNumber as u64
    } else {
        0
    };

    // u64 storage keeps the 8-byte alignment FILE_ID_BOTH_DIR_INFO needs
    let mut buf = vec![0u64; BUF_LEN / 8];
    let mut class = FileIdBothDirectoryRestartInfo;
    let mut first = true;
    loop {
        let ok = unsafe {
            GetFileInformationByHandleEx(
                h.0,
                class,
                buf.as_mut_ptr().cast(),
                (buf.len() * 8) as u32,
            )
        };
        class = FileIdBothDirectoryInfo;
        if ok == 0 {
            let code = unsafe { GetLastError() };
            if code == ERROR_NO_MORE_FILES {
                return Ok(());
            }
            if first && (code == ERROR_INVALID_PARAMETER || code == ERROR_NOT_SUPPORTED) {
                return Err(io::Error::from(io::ErrorKind::Unsupported));
            }
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        first = false;
        let base = buf.as_ptr().cast::<u8>();
        let mut off = 0usize;
        loop {
            // SAFETY: the system wrote a chain of FILE_ID_BOTH_DIR_INFO records into buf
            let rec = unsafe { &*(base.add(off).cast::<FILE_ID_BOTH_DIR_INFO>()) };
            let name_len = rec.FileNameLength as usize / 2;
            let name_ptr = std::ptr::addr_of!(rec.FileName).cast::<u16>();
            let name = unsafe { std::slice::from_raw_parts(name_ptr, name_len) };
            if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                visit(&entry_of(rec, OsString::from_wide(name), volume));
            }
            if rec.NextEntryOffset == 0 {
                break;
            }
            off += rec.NextEntryOffset as usize;
        }
    }
}

fn entry_of(rec: &FILE_ID_BOTH_DIR_INFO, name: OsString, volume: u64) -> Entry {
    let attrs = rec.FileAttributes;
    // symlinks, junctions and volume mount points are all reparse points: never followed
    let kind = if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        Kind::Symlink
    } else if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
        Kind::Dir
    } else {
        Kind::File
    };
    Entry {
        kind,
        dev: volume,
        ino: rec.FileId as u64,
        alloc: rec.AllocationSize.max(0) as u64,
        len: rec.EndOfFile.max(0) as u64,
        mtime: filetime_to_unix(rec.LastWriteTime as u64),
        atime: filetime_to_unix(rec.LastAccessTime as u64),
        ..Entry::empty(name)
    }
}
