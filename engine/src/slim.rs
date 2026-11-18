//! Slim Apps (macOS on Apple Silicon): drop the Intel code from universal apps.
//!
//! The app is cloned (APFS clone: no extra space), every universal Mach-O file in the
//! clone is cut down to its arm64 slice, and the clone must pass `codesign --verify
//! --deep --strict` (the original too, first). Only then are the two swapped by rename and
//! the original goes to the Trash, so it can be put back.

use crate::apps::App;
use crate::guard::Purpose;
use crate::picks::{Action, Pick};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const FAT_MAGIC: u32 = 0xCAFE_BABE;
const FAT_MAGIC_64: u32 = 0xCAFE_BABF;
const CPU_TYPE_ARM64: u32 = 0x0100_000C;
const CPU_ARCH_ABI64: u32 = 0x0100_0000;
/// <sys/clonefile.h>; not in libc
const CLONE_NOFOLLOW: u32 = 0x0001;

/// A universal file: where its arm64 slice is, and how many bytes the others take.
#[derive(Debug, PartialEq, Eq)]
pub struct Fat {
    pub arm64_offset: u64,
    pub arm64_size: u64,
    pub other_bytes: u64,
}

/// Reads a fat Mach-O header. None for anything else, including Java class files (same
/// magic, but their "architecture count" is a class version) and fat files with no arm64.
pub fn read_fat(path: &Path) -> io::Result<Option<Fat>> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut head = [0u8; 8];
    if f.read(&mut head)? < 8 {
        return Ok(None);
    }
    let magic = u32::from_be_bytes(head[0..4].try_into().unwrap());
    let count = u32::from_be_bytes(head[4..8].try_into().unwrap());
    let wide = match magic {
        FAT_MAGIC => false,
        FAT_MAGIC_64 => true,
        _ => return Ok(None),
    };
    if count == 0 || count > 8 {
        return Ok(None);
    }
    let entry = if wide { 32 } else { 20 };
    let mut table = vec![0u8; entry * count as usize];
    f.read_exact(&mut table)?;
    let be32 = |b: &[u8]| u32::from_be_bytes(b[..4].try_into().unwrap());
    let be64 = |b: &[u8]| u64::from_be_bytes(b[..8].try_into().unwrap());
    let mut arm = None;
    let mut total = 0u64;
    for e in table.chunks(entry) {
        let cpu = be32(&e[0..]);
        let (offset, size) = if wide {
            (be64(&e[8..]), be64(&e[16..]))
        } else {
            (be32(&e[8..]) as u64, be32(&e[12..]) as u64)
        };
        // every slice must be a real Mach-O CPU and lie inside the file
        if cpu & !CPU_ARCH_ABI64 > 0xFF
            || offset.checked_add(size).is_none_or(|end| end > len)
            || size == 0
        {
            return Ok(None);
        }
        total += size;
        if cpu == CPU_TYPE_ARM64 {
            arm = Some((offset, size));
        }
    }
    Ok(arm.map(|(arm64_offset, arm64_size)| Fat {
        arm64_offset,
        arm64_size,
        other_bytes: total - arm64_size,
    }))
}

/// Universal files in a bundle (not following links), with their header.
fn fat_files(bundle: &Path) -> Vec<(PathBuf, Fat)> {
    let mut out = Vec::new();
    let mut stack = vec![bundle.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(items) = fs::read_dir(&dir) else {
            continue;
        };
        for item in items.flatten() {
            let Ok(meta) = item.metadata() else { continue };
            let path = item.path();
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file()
                && meta.len() > 4096
                && looks_like_code(&path, &meta)
                && let Ok(Some(fat)) = read_fat(&path)
                && fat.other_bytes > 0
            {
                out.push((path, fat));
            }
        }
    }
    out
}

/// Executables, libraries and plug-in binaries; skips the many resource files.
fn looks_like_code(path: &Path, meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
        || path
            .extension()
            .is_some_and(|e| e == "dylib" || e == "so" || e == "a" || e == "node")
}

/// Universal apps the user may slim, with the bytes it would save.
pub fn candidates(apps: &[App]) -> Vec<Pick> {
    if !cfg!(target_arch = "aarch64") {
        return Vec::new();
    }
    let mut picks = Vec::new();
    for app in apps {
        if app.bundle_id.starts_with("com.apple.") || app.path.join("Contents/_MASReceipt").exists()
        {
            continue; // Apple's own and App Store apps: signed by Apple, updated by the store
        }
        let saving: u64 = fat_files(&app.path)
            .iter()
            .map(|(_, f)| f.other_bytes)
            .sum();
        if saving < 1 << 20 {
            continue;
        }
        let root = app.path.parent().unwrap_or(Path::new("/")).to_path_buf();
        let mut pick = Pick::new(app.path.clone(), root, Purpose::Uninstall, Action::Slim);
        pick.group = String::new();
        pick.bytes = saving;
        pick.detail = if app.running {
            "Open now: quit it first".into()
        } else {
            "Intel code".into()
        };
        pick.preselect = !app.running && writable(&app.path);
        pick.running = vec![app.bundle_id.clone()];
        picks.push(pick);
    }
    picks.sort_by_key(|p| std::cmp::Reverse(p.bytes));
    picks
}

fn writable(path: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Some(parent) = path.parent() else {
        return false;
    };
    let ok = |p: &Path| {
        CString::new(p.as_os_str().as_bytes())
            .is_ok_and(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
    };
    ok(parent) && ok(path)
}

fn verify(path: &Path) -> bool {
    Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(path)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Thins one app in place (see the module comment). The original ends up in the Trash.
pub fn slim_app(app: &Path) -> io::Result<()> {
    if !cfg!(target_arch = "aarch64") {
        return Err(io::Error::other("only on Apple Silicon"));
    }
    if !writable(app) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "needs administrator rights; not changed",
        ));
    }
    if !verify(app) {
        return Err(io::Error::other(
            "its signature does not verify as it is; not changed",
        ));
    }
    let parent = app
        .parent()
        .ok_or_else(|| io::Error::other("no parent folder"))?;
    let name = app
        .file_name()
        .ok_or_else(|| io::Error::other("no name"))?
        .to_string_lossy()
        .into_owned();
    let clone = parent.join(format!(".{name}.oc-slim"));
    let old = parent.join(format!(".{name}.oc-original"));
    for leftover in [&clone, &old] {
        if fs::symlink_metadata(leftover).is_ok() {
            fs::remove_dir_all(leftover)?;
        }
    }
    clonefile(app, &clone)?;
    let result = (|| {
        for (path, fat) in fat_files(&clone) {
            thin(&path, &fat)?;
        }
        if !verify(&clone) {
            return Err(io::Error::other("its signature would break; not changed"));
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&clone);
        return Err(e);
    }
    fs::rename(app, &old)?;
    if let Err(e) = fs::rename(&clone, app) {
        let _ = fs::rename(&old, app);
        let _ = fs::remove_dir_all(&clone);
        return Err(e);
    }
    crate::trash::move_to_trash(&old).or_else(|_| fs::remove_dir_all(&old))
}

/// Replaces a universal file with its arm64 slice, keeping its permissions.
fn thin(path: &Path, fat: &Fat) -> io::Result<()> {
    let perms = fs::metadata(path)?.permissions();
    let mut slice = vec![0u8; fat.arm64_size as usize];
    let mut f = File::open(path)?;
    f.seek(SeekFrom::Start(fat.arm64_offset))?;
    f.read_exact(&mut slice)?;
    drop(f);
    let tmp = path.with_extension("oc-thin");
    {
        let mut out = File::create(&tmp)?;
        out.write_all(&slice)?;
        out.sync_all()?;
    }
    fs::set_permissions(&tmp, perms)?;
    fs::rename(&tmp, path)
}

fn clonefile(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let a = CString::new(from.as_os_str().as_bytes())?;
    let b = CString::new(to.as_os_str().as_bytes())?;
    if unsafe { libc::clonefile(a.as_ptr(), b.as_ptr(), CLONE_NOFOLLOW) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fat_bytes(archs: &[(u32, u32)], pad: usize) -> Vec<u8> {
        // header, table, then each slice at a 4 KiB boundary filled with its cpu byte
        let mut out = Vec::new();
        out.extend(FAT_MAGIC.to_be_bytes());
        out.extend((archs.len() as u32).to_be_bytes());
        let mut offset = 4096u32;
        for (cpu, size) in archs {
            out.extend(cpu.to_be_bytes());
            out.extend(0u32.to_be_bytes());
            out.extend(offset.to_be_bytes());
            out.extend(size.to_be_bytes());
            out.extend(12u32.to_be_bytes());
            offset += size.div_ceil(4096) * 4096;
        }
        out.resize(4096, 0);
        for (cpu, size) in archs {
            let start = out.len();
            out.resize(start + *size as usize, (*cpu & 0xff) as u8);
            out.resize(start + (*size as usize).div_ceil(4096) * 4096, 0);
        }
        out.resize(out.len() + pad, 0);
        out
    }

    #[test]
    fn reads_fat_headers_and_thins_to_arm64() {
        let dir = std::env::temp_dir().join(format!("oc-slim-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("app");
        fs::write(
            &bin,
            fat_bytes(&[(0x0100_0007, 10_000), (CPU_TYPE_ARM64, 9_000)], 0),
        )
        .unwrap();
        let fat = read_fat(&bin).unwrap().unwrap();
        assert_eq!((fat.arm64_size, fat.other_bytes), (9_000, 10_000));
        thin(&bin, &fat).unwrap();
        let thinned = fs::read(&bin).unwrap();
        assert_eq!(thinned.len(), 9_000);
        assert!(thinned.iter().all(|&b| b == (CPU_TYPE_ARM64 & 0xff) as u8));

        // Intel-only fat file: nothing to keep, not a candidate
        fs::write(
            &bin,
            fat_bytes(&[(0x0100_0007, 5000), (0x0000_0007, 5000)], 0),
        )
        .unwrap();
        assert_eq!(read_fat(&bin).unwrap(), None);
        // a Java class file shares the magic; its "count" is a class version
        let mut class = vec![0xCA, 0xFE, 0xBA, 0xBE, 0x00, 0x00, 0x00, 0x34];
        class.resize(5000, 0);
        fs::write(&bin, class).unwrap();
        assert_eq!(read_fat(&bin).unwrap(), None);
        // a slice pointing past the end of the file
        let mut broken = fat_bytes(&[(CPU_TYPE_ARM64, 9_000)], 0);
        broken.truncate(6000);
        fs::write(&bin, broken).unwrap();
        assert_eq!(read_fat(&bin).unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}

/// Builds a real universal app, signs it ad hoc and slims it. Uses the real Trash for the
/// original (and takes it back out): run with `cargo test -- --ignored slim_real`.
#[cfg(all(test, target_arch = "aarch64"))]
mod real {
    use super::*;

    #[test]
    #[ignore]
    fn slim_real_app_keeps_a_valid_signature_and_trashes_the_original() {
        let dir = std::env::temp_dir().join(format!("oc-slim-real-{}", std::process::id()));
        let app = dir.join("OCSlimTest.app");
        let macos = app.join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        fs::write(dir.join("main.c"), "int main(void){return 0;}\n").unwrap();
        let built = Command::new("/usr/bin/clang")
            .args(["-arch", "arm64", "-arch", "x86_64", "-o"])
            .arg(macos.join("OCSlimTest"))
            .arg(dir.join("main.c"))
            .status()
            .unwrap();
        assert!(built.success());
        fs::write(
            app.join("Contents/Info.plist"),
            r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>OCSlimTest</string>
<key>CFBundleIdentifier</key><string>io.github.opencleaner.slimtest</string>
<key>CFBundlePackageType</key><string>APPL</string></dict></plist>"#,
        )
        .unwrap();
        assert!(
            Command::new("/usr/bin/codesign")
                .args(["-s", "-", "--force", "--deep"])
                .arg(&app)
                .status()
                .unwrap()
                .success()
        );
        let before = read_fat(&macos.join("OCSlimTest")).unwrap().unwrap();
        assert!(before.other_bytes > 0);

        slim_app(&app).unwrap();

        let exe = fs::read(macos.join("OCSlimTest")).unwrap();
        assert_eq!(&exe[..4], &[0xCF, 0xFA, 0xED, 0xFE], "thin arm64 Mach-O");
        assert!(verify(&app), "signature still valid");
        assert!(
            Command::new(macos.join("OCSlimTest"))
                .status()
                .unwrap()
                .success(),
            "still runs"
        );
        assert!(
            !dir.join(".OCSlimTest.app.oc-slim").exists()
                && !dir.join(".OCSlimTest.app.oc-original").exists()
        );
        // the original went to the Trash: take it back out
        let trash = Path::new(&std::env::var("HOME").unwrap()).join(".Trash");
        let trashed: Vec<_> = fs::read_dir(&trash)
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(".OCSlimTest.app.oc-original")
            })
            .collect();
        assert_eq!(trashed.len(), 1, "the original is in the Trash");
        for t in trashed {
            fs::remove_dir_all(t.path()).unwrap();
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
