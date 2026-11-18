//! macOS listing with `getattrlistbulk(2)`: one call returns name, type, ids, sizes and
//! times for as many entries as fit in the buffer.

use super::{Detail, Entry, Kind};
use libc::{attrgroup_t, c_void};
use std::cell::RefCell;
use std::ffi::{CString, OsString};
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;

// Requested attributes. The kernel packs the returned ones in bit order within each
// group, ATTR_CMN_RETURNED_ATTRS first and ATTR_CMN_ERROR right after it.
const ATTR_CMN_ERROR: attrgroup_t = 0x2000_0000; // <sys/attr.h>, not in libc
const COMMON: attrgroup_t = libc::ATTR_CMN_RETURNED_ATTRS
    | ATTR_CMN_ERROR
    | libc::ATTR_CMN_NAME
    | libc::ATTR_CMN_DEVID
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_ACCTIME
    | libc::ATTR_CMN_FLAGS
    | libc::ATTR_CMN_FILEID;
const DIR: attrgroup_t =
    libc::ATTR_DIR_LINKCOUNT | libc::ATTR_DIR_MOUNTSTATUS | libc::ATTR_DIR_ALLOCSIZE;
const FILE: attrgroup_t =
    libc::ATTR_FILE_LINKCOUNT | libc::ATTR_FILE_ALLOCSIZE | libc::ATTR_FILE_DATALENGTH;
// with FSOPT_ATTR_CMN_EXTENDED the fork group carries extended common attributes
const EXTENDED: attrgroup_t = libc::ATTR_CMNEXT_PRIVATESIZE;
const OPTIONS: u32 = libc::FSOPT_ATTR_CMN_EXTENDED;

const UF_COMPRESSED: u32 = 0x20;
// enum vtype in <sys/vnode.h>
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VLNK: u32 = 5;

const BUF_LEN: usize = 128 * 1024;

thread_local! {
    static BUF: RefCell<Vec<u8>> = RefCell::new(vec![0; BUF_LEN]);
}

fn attr_list(detail: Detail) -> libc::attrlist {
    libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: COMMON,
        volattr: 0,
        dirattr: DIR,
        fileattr: FILE,
        forkattr: if detail == Detail::Freed { EXTENDED } else { 0 },
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))
}

struct Fd(i32);
impl Drop for Fd {
    fn drop(&mut self) {
        unsafe { libc::close(self.0) };
    }
}

/// Lists `dir`. Returns `ErrorKind::Unsupported` before visiting anything when the volume
/// has no `getattrlistbulk` (the caller then falls back to readdir + lstat).
pub fn read_dir(dir: &Path, detail: Detail, visit: &mut dyn FnMut(&Entry)) -> io::Result<()> {
    let path = c_path(dir)?;
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = Fd(fd);
    let mut attrs = attr_list(detail);
    BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        let mut first = true;
        loop {
            let n = unsafe {
                libc::getattrlistbulk(
                    fd.0,
                    (&mut attrs as *mut libc::attrlist).cast::<c_void>(),
                    buf.as_mut_ptr().cast::<c_void>(),
                    buf.len(),
                    OPTIONS as u64,
                )
            };
            if n < 0 {
                let err = io::Error::last_os_error();
                match err.raw_os_error() {
                    Some(libc::EINTR) => continue,
                    Some(libc::ENOTSUP | libc::EINVAL | libc::ENOSYS) if first => {
                        return Err(io::Error::from(io::ErrorKind::Unsupported));
                    }
                    _ => return Err(err),
                }
            }
            first = false;
            if n == 0 {
                return Ok(());
            }
            let mut off = 0usize;
            for _ in 0..n {
                let Some(len) = read_u32(&buf, off) else {
                    break;
                };
                let len = len as usize;
                if len < 4 || off + len > buf.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "getattrlistbulk: bad entry length",
                    ));
                }
                if let Some(entry) = parse(&buf[off..off + len]) {
                    visit(&entry);
                }
                off += len;
            }
        }
    })
}

/// The entry for `path` itself (final symlink not followed).
pub fn stat(path: &Path, detail: Detail) -> io::Result<Entry> {
    let c = c_path(path)?;
    let mut attrs = attr_list(detail);
    let mut buf = [0u8; 2048];
    let rc = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            (&mut attrs as *mut libc::attrlist).cast::<c_void>(),
            buf.as_mut_ptr().cast::<c_void>(),
            buf.len(),
            libc::FSOPT_NOFOLLOW | OPTIONS,
        )
    };
    if rc != 0 {
        let err = io::Error::last_os_error();
        return match err.raw_os_error() {
            Some(libc::ENOTSUP | libc::EINVAL) => Err(io::Error::from(io::ErrorKind::Unsupported)),
            _ => Err(err),
        };
    }
    let len = (read_u32(&buf, 0).unwrap_or(0) as usize).min(buf.len());
    let mut entry = parse(&buf[..len])
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "getattrlist: bad reply"))?;
    // the root of a volume has no usable name; callers want the path's last component
    entry.name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or(entry.name);
    if let Some(code) = entry.error {
        return Err(io::Error::from_raw_os_error(code));
    }
    Ok(entry)
}

fn read_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn read_u64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
}

/// Decodes one packed entry, reading only the attributes the kernel says it returned.
fn parse(e: &[u8]) -> Option<Entry> {
    let mut at = 4; // entry length
    let mut u32_ = || {
        let v = read_u32(e, at);
        at += 4;
        v
    };
    let common = u32_()?;
    let _vol = u32_()?;
    let dir = u32_()?;
    let file = u32_()?;
    let ext = u32_()?;

    let mut entry = Entry::empty(OsString::new());
    let mut cur = at;
    let take32 = |cur: &mut usize| {
        let v = read_u32(e, *cur);
        *cur += 4;
        v
    };
    if common & ATTR_CMN_ERROR != 0 {
        let code = take32(&mut cur)?;
        if code != 0 {
            entry.error = Some(code as i32);
        }
    }
    if common & libc::ATTR_CMN_NAME != 0 {
        let field = cur;
        let off = take32(&mut cur)? as i32;
        let len = take32(&mut cur)? as usize;
        let start = field.checked_add_signed(off as isize)?;
        if len > 1 && start + len <= e.len() {
            entry.name = OsString::from_vec(e[start..start + len - 1].to_vec()); // drop the NUL
        }
    }
    if entry.name.is_empty() {
        return None;
    }
    if entry.error.is_some() {
        return Some(entry);
    }
    if common & libc::ATTR_CMN_DEVID != 0 {
        entry.dev = take32(&mut cur)? as i32 as u32 as u64;
    }
    let mut obj = 0;
    if common & libc::ATTR_CMN_OBJTYPE != 0 {
        obj = take32(&mut cur)?;
    }
    if common & libc::ATTR_CMN_MODTIME != 0 {
        entry.mtime = read_u64(e, cur)? as i64;
        cur += 16;
    }
    if common & libc::ATTR_CMN_ACCTIME != 0 {
        entry.atime = read_u64(e, cur)? as i64;
        cur += 16;
    }
    if common & libc::ATTR_CMN_FLAGS != 0 {
        entry.compressed = take32(&mut cur)? & UF_COMPRESSED != 0;
    }
    if common & libc::ATTR_CMN_FILEID != 0 {
        entry.ino = read_u64(e, cur)?;
        cur += 8;
    }
    entry.kind = match obj {
        VREG => Kind::File,
        VDIR => Kind::Dir,
        VLNK => Kind::Symlink,
        _ => Kind::Other,
    };
    if entry.kind == Kind::Dir {
        if dir & libc::ATTR_DIR_LINKCOUNT != 0 {
            entry.nlink = take32(&mut cur)?;
        }
        if dir & libc::ATTR_DIR_MOUNTSTATUS != 0 {
            entry.mount_point = take32(&mut cur)? & libc::DIR_MNTSTATUS_MNTPOINT != 0;
        }
        if dir & libc::ATTR_DIR_ALLOCSIZE != 0 {
            entry.alloc = read_u64(e, cur)?;
            cur += 8;
        }
    } else {
        if file & libc::ATTR_FILE_LINKCOUNT != 0 {
            entry.nlink = take32(&mut cur)?;
        }
        if file & libc::ATTR_FILE_ALLOCSIZE != 0 {
            entry.alloc = read_u64(e, cur)?;
            cur += 8;
        }
        if file & libc::ATTR_FILE_DATALENGTH != 0 {
            entry.len = read_u64(e, cur)?;
            cur += 8;
        }
    }
    if ext & libc::ATTR_CMNEXT_PRIVATESIZE != 0 {
        entry.private = Some(read_u64(e, cur)?);
    }
    Some(entry)
}
