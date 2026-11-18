//! Moving to the Trash (macOS) or Recycle Bin (Windows) the way the system does it, so the
//! user can put things back from Finder / Explorer.

use std::io;
use std::path::Path;

#[cfg(target_os = "macos")]
pub fn move_to_trash(path: &Path) -> io::Result<()> {
    mac::trash(path)
}

#[cfg(windows)]
pub fn move_to_trash(path: &Path) -> io::Result<()> {
    win::trash(path)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn move_to_trash(_path: &Path) -> io::Result<()> {
    Err(io::Error::from(io::ErrorKind::Unsupported))
}

/// `-[NSFileManager trashItemAtURL:resultingItemURL:error:]` through the Objective-C
/// runtime (four messages; no binding crate needed).
#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::{CString, c_char, c_void};
    use std::io;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

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

    pub fn trash(path: &Path) -> io::Result<()> {
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
            let result = if ok && std::fs::symlink_metadata(path).is_ok() {
                // What it cannot remove (an item on a read-only or protected volume) is
                // *copied* to the Trash and reported as trashed. Undo the copy and say so.
                if !resulting.is_null() {
                    let copy_path = send(resulting, sel("path"));
                    let text = if copy_path.is_null() {
                        std::ptr::null()
                    } else {
                        utf8(copy_path, sel("UTF8String"))
                    };
                    if !text.is_null() {
                        let copy = std::ffi::CStr::from_ptr(text)
                            .to_string_lossy()
                            .into_owned();
                        super::remove_trash_copy(Path::new(&copy));
                    }
                }
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "it cannot be moved (read-only or protected)",
                ))
            } else if ok {
                Ok(())
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
    let in_trash = copy.ancestors().skip(1).any(|a| {
        a.file_name()
            .is_some_and(|n| n == ".Trash" || n == ".Trashes")
    });
    if !in_trash {
        return;
    }
    let _ = match std::fs::symlink_metadata(copy) {
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(copy),
        Ok(_) => std::fs::remove_file(copy),
        Err(_) => Ok(()),
    };
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };

    pub fn trash(path: &Path) -> io::Result<()> {
        // double-NUL-terminated list of one path
        let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
        from.extend([0, 0]);
        let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
        op.wFunc = FO_DELETE;
        op.pFrom = from.as_ptr();
        op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as u16;
        let rc = unsafe { SHFileOperationW(&mut op) };
        if rc != 0 || op.fAnyOperationsAborted != 0 {
            return Err(io::Error::other(format!(
                "could not move to the Recycle Bin (error {rc})"
            )));
        }
        Ok(())
    }
}
