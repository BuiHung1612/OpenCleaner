//! Slim Apps (macOS on Apple Silicon): drop the Intel code from universal apps.
//!
//! The app is cloned (APFS clone: no extra space), every universal Mach-O file in the
//! clone is cut down to its arm64 slice, and the clone must pass `codesign --verify
//! --deep --strict` (the original too, first). Files sealed as plain resources (loose
//! dylibs inside some frameworks) break the seal when cut: codesign names them, they are
//! put back from the original, and the check runs again. Only then are the two swapped
//! atomically (`renamex_np` RENAME_SWAP) and the original goes to the Trash, so it can be
//! put back; if it cannot, it is kept next to the app, never deleted.

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
/// magic, but their "architecture count" is a class version) and fat files with no plain
/// arm64 slice (arm64e alone is left as it is).
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
        let subtype = be32(&e[4..]);
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
        // plain arm64 only (CPU_SUBTYPE_ARM64_ALL): an arm64e slice would still verify,
        // but third-party code built for arm64 then fails to load in it
        if cpu == CPU_TYPE_ARM64 && subtype & 0x00FF_FFFF == 0 {
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
        // installed by a .pkg (owned by root): listed, never ticked for the user, and said why
        let can_write = writable(&app.path);
        pick.detail = if !can_write && app.running {
            "Open now · needs administrator rights".into()
        } else if !can_write {
            "Needs administrator rights".into()
        } else if app.running {
            "Open now: quit it first".into()
        } else {
            "Intel code".into()
        };
        pick.preselect = !app.running && can_write;
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
    check(path).is_ok()
}

/// codesign's verdict: Ok, or what it blames: files modified or missing, or a nested
/// bundle ("In subcomponent") without naming files (empty when it failed for another reason).
fn check(path: &Path) -> Result<(), Vec<PathBuf>> {
    let Ok(out) = Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict", "-vvvv"]) // -vvvv names the files that break the seal
        .arg(path)
        .output()
    else {
        return Err(Vec::new());
    };
    if out.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&out.stderr)
        .lines()
        .filter_map(|l| {
            l.strip_prefix("file modified: ")
                .or_else(|| l.strip_prefix("file missing: "))
                .or_else(|| l.strip_prefix("In subcomponent: "))
        })
        .map(PathBuf::from)
        .collect())
}

/// Thins one app in place (see the module comment) and returns the bytes it saved and
/// where the original went in the Trash (as "X.app", so Put Back works).
pub fn slim_app(app: &Path) -> io::Result<(u64, Option<PathBuf>)> {
    if !cfg!(target_arch = "aarch64") {
        return Err(io::Error::other("only on Apple Silicon"));
    }
    let parent = app
        .parent()
        .ok_or_else(|| io::Error::other("no parent folder"))?;
    let name = app
        .file_name()
        .ok_or_else(|| io::Error::other("no name"))?
        .to_string_lossy()
        .into_owned();
    recover(app, parent, &name)?;
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
    // the clone lives in a hidden work folder under the app's own name, so after the swap
    // the original goes to the Trash as "X.app"
    let work = parent.join(format!(".{name}.oc-work"));
    fs::create_dir(&work)?;
    let clone = work.join(&name);
    if let Err(e) = clonefile(app, &clone) {
        let _ = fs::remove_dir_all(&work);
        return Err(e);
    }
    let result = (|| {
        let mut saved: std::collections::HashMap<PathBuf, u64> = std::collections::HashMap::new();
        for (path, fat) in fat_files(&clone) {
            thin(&path, &fat)?;
            saved.insert(path, fat.other_bytes);
        }
        // put back the thinned files codesign blames (a file, or everything thinned inside a
        // nested bundle), one round per broken seal
        for _ in 0..16 {
            let Err(blamed) = check(&clone) else { break };
            // codesign names a file, or only the bundle whose seal broke (or nothing, for the
            // app itself): then the suspects are the thinned files that bundle seals as plain
            // resources, never its own executable, which carries its own signature
            let roots: Vec<PathBuf> = if blamed.is_empty() {
                vec![clone.clone()]
            } else {
                blamed
            };
            let back: Vec<PathBuf> = saved
                .keys()
                .filter(|f| roots.iter().any(|b| *f == b || sealed_by(b, f)))
                .cloned()
                .collect();
            if back.is_empty() {
                break;
            }
            for f in back {
                saved.remove(&f);
                let rel = f.strip_prefix(&clone).map_err(io::Error::other)?;
                fs::remove_file(&f)?;
                clonefile(&app.join(rel), &f)?;
            }
        }
        if saved.is_empty() || !verify(&clone) {
            return Err(io::Error::other("its signature would break; not changed"));
        }
        Ok(saved.values().sum::<u64>())
    })();
    let saved = match result.and_then(|saved| swap(&clone, app).map(|()| saved)) {
        Ok(saved) => saved,
        Err(e) => {
            let _ = fs::remove_dir_all(&work); // only the clone: the swap did not happen
            return Err(e);
        }
    };
    // `clone` now holds the original
    match crate::trash::move_to_trash(&clone) {
        Ok(trashed) => {
            let _ = fs::remove_dir(&work);
            Ok((saved, trashed))
        }
        Err(e) => {
            // never delete the original: keep it next to the app, visible
            let stem = Path::new(&name)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy();
            let kept = parent.join(format!("{stem} (original).app"));
            let at = if fs::symlink_metadata(&kept).is_err() && fs::rename(&clone, &kept).is_ok() {
                let _ = fs::remove_dir(&work);
                kept
            } else {
                clone
            };
            Err(io::Error::other(format!(
                "slimmed, but the original could not go to the Trash ({e}); it was kept at {}",
                at.display()
            )))
        }
    }
}

/// Leftovers of an interrupted run. Never deletes an original: one is put back when the
/// app is missing, else moved to the Trash.
fn recover(app: &Path, parent: &Path, name: &str) -> io::Result<()> {
    let exists = |p: &Path| fs::symlink_metadata(p).is_ok();
    // an older version's clone: only ever a copy
    let old_clone = parent.join(format!(".{name}.oc-slim"));
    if exists(&old_clone) {
        fs::remove_dir_all(&old_clone)?;
    }
    // this version's work folder: the clone, or the original if the trash step was cut
    // short; either way the app itself is in place (the swap is atomic)
    let work = parent.join(format!(".{name}.oc-work"));
    let inside = work.join(name);
    if exists(&inside) {
        crate::trash::move_to_trash(&inside)?;
    }
    if exists(&work) {
        fs::remove_dir(&work)?;
    }
    // an older version's original (it swapped with two renames)
    let old = parent.join(format!(".{name}.oc-original"));
    if exists(&old) {
        if !exists(app) {
            fs::rename(&old, app)?;
        } else {
            fs::create_dir(&work)?;
            fs::rename(&old, &inside)?;
            crate::trash::move_to_trash(&inside)?;
            fs::remove_dir(&work)?;
        }
    }
    Ok(())
}

/// Swaps two items atomically: a crash leaves either the old or the new app in place.
fn swap(a: &Path, b: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let a = CString::new(a.as_os_str().as_bytes())?;
    let b = CString::new(b.as_os_str().as_bytes())?;
    if unsafe { libc::renamex_np(a.as_ptr(), b.as_ptr(), libc::RENAME_SWAP) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// A file `bundle` seals as a plain resource: inside it, not inside a bundle nested in it,
/// and not its executable (`Contents/MacOS/*` of an app, `Versions/*/<Name>` of a framework).
fn sealed_by(bundle: &Path, file: &Path) -> bool {
    let Ok(rel) = file.strip_prefix(bundle) else {
        return false;
    };
    let parts: Vec<&std::ffi::OsStr> = rel.iter().collect();
    let is_bundle = |c: &std::ffi::OsStr| {
        Path::new(c).extension().is_some_and(|e| {
            ["app", "framework", "bundle", "xpc", "appex", "plugin"]
                .contains(&e.to_string_lossy().as_ref())
        })
    };
    if parts
        .iter()
        .take(parts.len().saturating_sub(1))
        .any(|c| is_bundle(c))
    {
        return false;
    }
    let name = bundle.file_stem().unwrap_or_default();
    let executable = match bundle.extension().and_then(|e| e.to_str()) {
        Some("framework") => {
            parts.last() == Some(&name)
                && (parts.len() == 1 || parts.first() == Some(&"Versions".as_ref()))
        }
        _ => parts.first() == Some(&"Contents".as_ref()) && parts.get(1) == Some(&"MacOS".as_ref()),
    };
    !executable
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
        // create_new: never follow a planted link or reuse a file already there
        let mut out = File::options().write(true).create_new(true).open(&tmp)?;
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
    fn plain_sealed_files_are_told_apart_from_executables_and_nested_bundles() {
        let app = Path::new("/A/X.app");
        assert!(sealed_by(
            app,
            Path::new("/A/X.app/Contents/Resources/updater.node")
        ));
        assert!(!sealed_by(app, Path::new("/A/X.app/Contents/MacOS/X")));
        assert!(!sealed_by(
            app,
            Path::new("/A/X.app/Contents/Frameworks/E.framework/Versions/A/Libraries/libEGL.dylib")
        ));
        let fw = Path::new("/A/X.app/Contents/Frameworks/E.framework");
        assert!(sealed_by(
            fw,
            Path::new("/A/X.app/Contents/Frameworks/E.framework/Versions/A/Libraries/libEGL.dylib")
        ));
        assert!(!sealed_by(
            fw,
            Path::new("/A/X.app/Contents/Frameworks/E.framework/Versions/A/E")
        ));
        assert!(!sealed_by(
            fw,
            Path::new(
                "/A/X.app/Contents/Frameworks/E.framework/Versions/A/Helpers/H.app/Contents/MacOS/H"
            )
        ));
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

    /// Sets the cpusubtype of table entry `i` (fat_bytes writes 0, ARM64_ALL).
    fn set_subtype(bytes: &mut [u8], i: usize, sub: u32) {
        let at = 8 + i * 20 + 4;
        bytes[at..at + 4].copy_from_slice(&sub.to_be_bytes());
    }

    #[test]
    fn picks_plain_arm64_never_arm64e() {
        let dir = std::env::temp_dir().join(format!("oc-slim-e-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("lib.dylib");
        // arm64e listed after arm64: the arm64 slice is kept, arm64e counts as other
        let mut both = fat_bytes(&[(CPU_TYPE_ARM64, 9_000), (CPU_TYPE_ARM64, 7_000)], 0);
        set_subtype(&mut both, 1, 2); // CPU_SUBTYPE_ARM64E
        fs::write(&bin, &both).unwrap();
        let fat = read_fat(&bin).unwrap().unwrap();
        assert_eq!(
            (fat.arm64_offset, fat.arm64_size, fat.other_bytes),
            (4096, 9_000, 7_000)
        );
        // arm64e (with capability bits) and Intel only: left untouched
        let mut e_only = fat_bytes(&[(0x0100_0007, 5000), (CPU_TYPE_ARM64, 5000)], 0);
        set_subtype(&mut e_only, 1, 0x8000_0002);
        fs::write(&bin, &e_only).unwrap();
        assert_eq!(read_fat(&bin).unwrap(), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn thin_does_not_follow_a_planted_link() {
        let dir = std::env::temp_dir().join(format!("oc-slim-l-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("app");
        fs::write(
            &bin,
            fat_bytes(&[(0x0100_0007, 5000), (CPU_TYPE_ARM64, 5000)], 0),
        )
        .unwrap();
        let target = dir.join("target");
        fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, bin.with_extension("oc-thin")).unwrap();
        let fat = read_fat(&bin).unwrap().unwrap();
        assert!(thin(&bin, &fat).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_leftover_original_is_put_back_when_the_app_is_missing() {
        let dir = std::env::temp_dir().join(format!("oc-slim-r-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".X.app.oc-original/Contents")).unwrap();
        fs::create_dir_all(dir.join(".X.app.oc-slim/Contents")).unwrap();
        fs::create_dir_all(dir.join(".X.app.oc-work")).unwrap(); // empty: cut short early
        let app = dir.join("X.app");
        recover(&app, &dir, "X.app").unwrap();
        assert!(app.join("Contents").is_dir(), "the original is back");
        for gone in [".X.app.oc-original", ".X.app.oc-slim", ".X.app.oc-work"] {
            assert!(!dir.join(gone).exists(), "{gone}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn swap_exchanges_two_folders() {
        let dir = std::env::temp_dir().join(format!("oc-slim-s-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("a/1"), b"").unwrap();
        swap(&dir.join("a"), &dir.join("b")).unwrap();
        assert!(dir.join("b/1").exists() && !dir.join("a/1").exists());
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

        let (_, trashed) = slim_app(&app).unwrap();

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
        assert!(!dir.join(".OCSlimTest.app.oc-work").exists());
        // the original went to the Trash under its own name: take it back out
        let trashed = trashed.expect("the original is in the Trash");
        assert!(
            trashed
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("OCSlimTest")
        );
        fs::remove_dir_all(trashed).unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }
}
