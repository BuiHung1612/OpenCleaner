//! Moving to the Trash (macOS) or Recycle Bin (Windows) the way the system does it, so the
//! user can put things back from Finder / Explorer.

use std::io;
use std::path::{Path, PathBuf};

/// Moves `path` to the Trash; returns where it went when the system says (macOS), so it
/// can be put back from History.
#[cfg(target_os = "macos")]
pub fn move_to_trash(path: &Path) -> io::Result<Option<PathBuf>> {
    // Given something it cannot remove, macOS copies it into the Trash and then reports
    // success or an error depending on the system: never ask it to move what cannot move.
    if !movable(path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "it cannot be moved (read-only or protected)",
        ));
    }
    let before = crate::guard::identity(path);
    let to = mac::trash(path)?;
    let now = crate::guard::identity(path);
    if now.is_none() {
        return Ok(to);
    }
    // Still there after "success". What it cannot remove (an item on a read-only or
    // protected volume) is *copied* to the Trash and reported as trashed: undo the copy,
    // but only when it is surely one (see is_trash_copy), never the real item.
    let in_trash = to.as_deref().and_then(crate::guard::identity);
    if let Some(copy) = to.as_deref()
        && is_trash_copy(before, now, in_trash)
    {
        remove_trash_copy(copy);
    } else if now != before {
        // it moved, and something new took its place since
        return Ok(to);
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "it cannot be moved (read-only or protected)",
    ))
}

/// After the system reported `path` trashed while the path still exists: is the item in
/// the Trash a copy to undo? Only when the path still holds the very object asked to move
/// (same dev/ino as before) and the Trash item is another object. If something recreated
/// the path in between, the Trash item may be the real one: it is never deleted.
#[cfg(any(target_os = "macos", test))]
fn is_trash_copy(
    before: Option<crate::guard::Identity>,
    now: Option<crate::guard::Identity>,
    in_trash: Option<crate::guard::Identity>,
) -> bool {
    match (before, now, in_trash) {
        (Some(b), Some(n), Some(t)) => b == n && (t.dev, t.ino) != (b.dev, b.ino),
        _ => false,
    }
}

/// Its folder accepts changes (write permission, not a read-only volume).
#[cfg(target_os = "macos")]
fn movable(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(c) = CString::new(parent.as_os_str().as_bytes()) else {
        return false;
    };
    let mut st: libc::statfs = unsafe { std::mem::zeroed() };
    let read_only = unsafe { libc::statfs(c.as_ptr(), &mut st) } != 0
        || st.f_flags & libc::MNT_RDONLY as u32 != 0;
    !read_only && unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0
}

#[cfg(windows)]
pub fn move_to_trash(path: &Path) -> io::Result<Option<PathBuf>> {
    win::trash(path).map(|()| None)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn move_to_trash(_path: &Path) -> io::Result<Option<PathBuf>> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// Inside a `.Trash` (home) or `.Trashes` (other volumes) folder.
fn in_trash(path: &Path) -> bool {
    path.ancestors().skip(1).any(|a| {
        a.file_name()
            .is_some_and(|n| n == ".Trash" || n == ".Trashes")
    })
}

/// Moves `trashed` (an item in a Trash folder) back to `original`, only if nothing is
/// there now: an existing file is never replaced.
pub fn put_back(original: &Path, trashed: &Path) -> io::Result<()> {
    let refuse = |why: &str| {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            why.to_string(),
        ))
    };
    if !crate::rules::is_clean_path(original) || !crate::rules::is_clean_path(trashed) {
        return refuse("not a plain absolute path");
    }
    if !in_trash(trashed) {
        return refuse("not in the Trash");
    }
    if std::fs::symlink_metadata(trashed).is_err() {
        return refuse("no longer in the Trash");
    }
    if std::fs::symlink_metadata(original).is_ok() {
        return refuse("something else is there now");
    }
    if !original.parent().is_some_and(|p| p.is_dir()) {
        return refuse("its folder is gone");
    }
    rename_new(trashed, original)
}

/// `put_back` of an item History recorded (`was`: its stamp in the Trash right after it
/// went there): refused when the Trash item now is another object or was changed, since
/// a name in the Trash says nothing about what it holds.
// ponytail: checked just before the rename; a swap in that instant is not caught
pub fn put_back_checked(
    original: &Path,
    trashed: &Path,
    was: &crate::guard::Stamp,
) -> io::Result<()> {
    if crate::guard::stamp(trashed).as_ref() != Some(was) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "not the item OpenCleaner moved to the Trash (replaced or changed)",
        ));
    }
    put_back(original, trashed)
}

/// A rename that fails when the target exists (no check-then-rename race).
#[cfg(target_os = "macos")]
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(io::Error::other);
    let (from, to) = (c(from)?, c(to)?);
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

// ponytail: checked just before in put_back; no Trash location is recorded off macOS anyway
#[cfg(not(target_os = "macos"))]
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::rename(from, to)
}

/// `-[NSFileManager trashItemAtURL:resultingItemURL:error:]` through the Objective-C
/// runtime (four messages; no binding crate needed).
#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CString, c_char, c_void};
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};

    type Id = *mut c_void;
    type Sel = *mut c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}

    fn class(name: &str) -> Id {
        let c = CString::new(name).unwrap();
        unsafe { objc_getClass(c.as_ptr()) }
    }

    fn sel(name: &str) -> Sel {
        let c = CString::new(name).unwrap();
        unsafe { sel_registerName(c.as_ptr()) }
    }

    pub fn trash(path: &Path) -> io::Result<Option<PathBuf>> {
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let send_ptr: unsafe extern "C" fn(Id, Sel, *const c_char) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_id: unsafe extern "C" fn(Id, Sel, Id) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send: unsafe extern "C" fn(Id, Sel) -> Id =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_trash: unsafe extern "C" fn(Id, Sel, Id, *mut Id, *mut Id) -> u8 =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());

            let string = send_ptr(
                class("NSString"),
                sel("stringWithUTF8String:"),
                c_path.as_ptr(),
            );
            let url = send_id(class("NSURL"), sel("fileURLWithPath:"), string);
            let manager = send(class("NSFileManager"), sel("defaultManager"));
            let mut error: Id = std::ptr::null_mut();
            let mut resulting: Id = std::ptr::null_mut();
            let ok = !string.is_null()
                && !url.is_null()
                && send_trash(
                    manager,
                    sel("trashItemAtURL:resultingItemURL:error:"),
                    url,
                    &mut resulting,
                    &mut error,
                ) != 0;
            let utf8: unsafe extern "C" fn(Id, Sel) -> *const c_char =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let resulting_path = || {
                if resulting.is_null() {
                    return None;
                }
                let p = send(resulting, sel("path"));
                let text = if p.is_null() {
                    std::ptr::null()
                } else {
                    utf8(p, sel("UTF8String"))
                };
                (!text.is_null()).then(|| {
                    PathBuf::from(
                        std::ffi::CStr::from_ptr(text)
                            .to_string_lossy()
                            .into_owned(),
                    )
                })
            };
            // a copy left behind instead of a move is undone by the caller
            let result = if ok {
                Ok(resulting_path())
            } else if !error.is_null() {
                let desc = send(error, sel("localizedDescription"));
                let text = utf8(desc, sel("UTF8String"));
                let msg = if text.is_null() {
                    "could not move to the Trash".to_string()
                } else {
                    std::ffi::CStr::from_ptr(text)
                        .to_string_lossy()
                        .into_owned()
                };
                Err(io::Error::other(msg))
            } else {
                Err(io::Error::other("could not move to the Trash"))
            };
            objc_autoreleasePoolPop(pool);
            result
        }
    }
}

/// Removes a copy the system left in a Trash folder, and nothing else: the path must be
/// inside a `.Trash` (home) or `.Trashes` (other volumes) folder.
#[cfg(target_os = "macos")]
fn remove_trash_copy(copy: &Path) {
    if !in_trash(copy) {
        return;
    }
    let _ = match std::fs::symlink_metadata(copy) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(copy),
        Ok(_) => std::fs::remove_file(copy),
        Err(_) => Ok(()),
    };
}

#[cfg(test)]
mod tests {
    use super::is_trash_copy;
    use crate::fs::Kind;
    use crate::guard::Identity;

    #[test]
    fn only_a_sure_copy_in_the_trash_is_undone() {
        let id = |ino| {
            Some(Identity {
                dev: 1,
                ino,
                kind: Kind::File,
            })
        };
        // the original never moved, the Trash holds another object: a copy
        assert!(is_trash_copy(id(5), id(5), id(9)));
        // something recreated the path: the Trash item may be the real one
        assert!(!is_trash_copy(id(5), id(7), id(5)));
        assert!(!is_trash_copy(id(5), id(7), id(9)));
        // the Trash item is the very object, or unknown: never deleted
        assert!(!is_trash_copy(id(5), id(5), id(5)));
        assert!(!is_trash_copy(id(5), id(5), None));
        assert!(!is_trash_copy(None, id(5), id(9)));
    }
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW,
    };
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT,
        FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW, SHFileOperationW, SHQUERYRBINFO, SHQueryRecycleBinW,
    };

    /// GetDriveTypeW: a hard disk (not removable, network, CD or RAM disk).
    const DRIVE_FIXED: u32 = 3;

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain([0]).collect()
    }

    fn dword(root: HKEY, key: &str, name: &str) -> Option<u32> {
        let (key, name) = (wide(key.as_ref()), wide(name.as_ref()));
        let (mut v, mut len) = (0u32, 4u32);
        let rc = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut v as *mut u32).cast(),
                &mut len,
            )
        };
        (rc == ERROR_SUCCESS).then_some(v)
    }

    /// Will the shell really recycle `path`? Asked with no UI, it deletes for good what the
    /// Recycle Bin cannot take (removable or network drive, bin turned off, item larger
    /// than the bin) and still reports success, so those cases are refused up front.
    // Unverified on a real Windows machine (compile-checked only): the registry layout
    // (BitBucket\Volume\{GUID}: NukeOnDelete, MaxCapacity in MB) is the documented
    // Explorer one; when MaxCapacity is absent (the default size) the size is not checked.
    fn recyclable(path: &Path) -> io::Result<()> {
        let refuse = |why: &str| -> io::Result<()> {
            Err(io::Error::other(format!(
                "cannot go to the Recycle Bin ({why})"
            )))
        };
        let p = wide(path.as_os_str());
        let mut root = [0u16; 1024];
        if unsafe { GetVolumePathNameW(p.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
            return refuse("unknown drive");
        }
        if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
            return refuse("not a fixed drive");
        }
        let mut info: SHQUERYRBINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<SHQUERYRBINFO>() as u32;
        if unsafe { SHQueryRecycleBinW(root.as_ptr(), &mut info) } != 0 {
            return refuse("this drive has none");
        }
        const POLICY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Policies\\Explorer";
        if [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE]
            .iter()
            .any(|&h| dword(h, POLICY, "NoRecycleFiles") == Some(1))
        {
            return refuse("turned off by policy");
        }
        let mut volume = [0u16; 64];
        if unsafe {
            GetVolumeNameForVolumeMountPointW(
                root.as_ptr(),
                volume.as_mut_ptr(),
                volume.len() as u32,
            )
        } == 0
        {
            return Ok(()); // no volume GUID: the shell's own defaults apply
        }
        let end = volume.iter().position(|&c| c == 0).unwrap_or(volume.len());
        let volume = String::from_utf16_lossy(&volume[..end]); // \\?\Volume{GUID}\
        let Some(guid) = volume
            .find('{')
            .and_then(|a| volume[a..].find('}').map(|b| &volume[a..=a + b]))
        else {
            return Ok(());
        };
        let key = format!(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\BitBucket\\Volume\\{guid}"
        );
        if dword(HKEY_CURRENT_USER, &key, "NukeOnDelete") == Some(1) {
            return refuse("turned off for this drive");
        }
        if let Some(mb) = dword(HKEY_CURRENT_USER, &key, "MaxCapacity") {
            let opts = crate::size::Options::default();
            let bytes = crate::size::measure(path, &opts).map_or(u64::MAX, |r| r.bytes);
            if bytes > u64::from(mb) << 20 {
                return refuse("larger than the Recycle Bin");
            }
        }
        Ok(())
    }

    pub fn trash(path: &Path) -> io::Result<()> {
        recyclable(path)?;
        // double-NUL-terminated list of one path
        let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
        from.extend([0, 0]);
        let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
        op.wFunc = FO_DELETE;
        op.pFrom = from.as_ptr();
        // FOF_WANTNUKEWARNING: should the bin still refuse it, ask before deleting for good
        // instead of doing it silently (it partly overrides FOF_NOCONFIRMATION). Unverified.
        op.fFlags =
            (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT | FOF_WANTNUKEWARNING)
                as u16;
        let rc = unsafe { SHFileOperationW(&mut op) };
        if rc != 0 || op.fAnyOperationsAborted != 0 {
            return Err(io::Error::other(format!(
                "could not move to the Recycle Bin (error {rc})"
            )));
        }
        Ok(())
    }
}
