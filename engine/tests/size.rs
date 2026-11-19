mod common;

use common::{TempDir, write};
use oc_engine::fs::{self, Detail, Kind};
use oc_engine::size::{self, Mode, Options};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn tree(root: &Path) {
    for i in 0..40 {
        write(&root.join(format!("a/{}/{i}.bin", i % 7)), 900 + i * 3001);
    }
    write(&root.join("big.bin"), 3 << 20);
    write(&root.join("deep/x/y/z/w/f.bin"), 12345);
}

fn opts(mode: Mode) -> Options {
    Options {
        mode,
        ..Default::default()
    }
}

/// The bulk listing must describe every entry the way readdir + lstat does.
#[test]
fn bulk_listing_matches_lstat() {
    let t = TempDir::new();
    tree(t.path());
    #[cfg(unix)]
    std::os::unix::fs::symlink("big.bin", t.path().join("link")).unwrap();
    let mut dirs = vec![t.path().to_path_buf(), t.path().join("a/3")];
    if let Some(extra) = std::env::var_os("OC_EXTRA_DIRS") {
        dirs.extend(std::env::split_paths(&extra));
    }
    for dir in dirs {
        let mut bulk = BTreeMap::new();
        fs::read_dir(&dir, Detail::Basic, &mut |e| {
            bulk.insert(e.name.clone(), e.clone());
        })
        .unwrap();
        let mut plain = BTreeMap::new();
        fs::read_dir_portable(&dir, &mut |e| {
            plain.insert(e.name.clone(), e.clone());
        })
        .unwrap();
        assert_eq!(
            bulk.keys().collect::<Vec<_>>(),
            plain.keys().collect::<Vec<_>>(),
            "{}",
            dir.display()
        );
        for (name, b) in &bulk {
            let p = &plain[name];
            if p.error.is_some() || b.error.is_some() {
                continue;
            }
            let what = format!("{}/{}", dir.display(), name.to_string_lossy());
            assert_eq!(b.kind, p.kind, "{what}");
            assert_eq!(b.mtime, p.mtime, "{what}");
            if b.kind != Kind::Dir {
                assert_eq!(b.len, p.len, "{what}");
            }
            if cfg!(unix) {
                assert_eq!(b.ino, p.ino, "{what}");
                assert_eq!(b.dev, p.dev, "{what}");
                assert_eq!(b.alloc, p.alloc, "{what}");
                if b.kind != Kind::Dir {
                    assert_eq!(b.nlink, p.nlink, "{what}");
                }
            }
        }
    }
}

#[test]
fn allocated_counts_files_dirs_and_hard_links_once() {
    let t = TempDir::new();
    tree(t.path());
    std::fs::hard_link(t.path().join("big.bin"), t.path().join("a/big-link")).unwrap();
    let r = size::measure(t.path(), &opts(Mode::Allocated)).unwrap();
    assert!(r.complete());
    assert_eq!(r.files, 43);
    // every byte written is allocated, the hard-linked 3 MiB only once
    let written: u64 = (0..40).map(|i| 900 + i * 3001).sum::<u64>() + (3 << 20) + 12345;
    assert!(r.bytes >= written, "{} < {written}", r.bytes);
    assert!(
        r.bytes < written + (3 << 20),
        "hard link counted twice: {}",
        r.bytes
    );
}

#[test]
fn freed_counts_a_hard_link_only_when_all_links_are_inside() {
    let t = TempDir::new();
    write(&t.path().join("d/inner"), 1 << 20);
    write(&t.path().join("outer"), 1 << 20);
    std::fs::hard_link(t.path().join("d/inner"), t.path().join("d/inner-2")).unwrap();
    std::fs::hard_link(t.path().join("outer"), t.path().join("d/outer-link")).unwrap();
    let r = size::measure(&t.path().join("d"), &opts(Mode::Freed)).unwrap();
    // inner (both links inside) counts once, outer-link frees nothing
    #[cfg(unix)]
    assert!(r.bytes >= 1 << 20 && r.bytes < 2 << 20, "{}", r.bytes);
    // Windows bulk listing has no link count: each file id counts once, so outer-link
    // counts too (see size.rs)
    #[cfg(windows)]
    assert!(r.bytes >= 2 << 20 && r.bytes < 3 << 20, "{}", r.bytes);
}

#[test]
fn roots_that_are_files_symlinks_or_missing() {
    let t = TempDir::new();
    write(&t.path().join("f"), 70000);
    let f = size::measure(&t.path().join("f"), &opts(Mode::Allocated)).unwrap();
    assert!(f.bytes >= 70000 && f.files == 1);
    assert!(size::measure(&t.path().join("missing"), &opts(Mode::Allocated)).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(t.path().join("f"), t.path().join("s")).unwrap();
        assert_eq!(
            size::measure(&t.path().join("s"), &opts(Mode::Freed))
                .unwrap()
                .bytes,
            0
        );
        assert!(
            size::measure(&t.path().join("s"), &opts(Mode::Allocated))
                .unwrap()
                .bytes
                < 70000
        );
    }
}

#[test]
fn cancelled_walk_stops_and_says_so() {
    let t = TempDir::new();
    tree(t.path());
    let cancel = AtomicBool::new(true);
    let r = size::measure_with(
        t.path(),
        &opts(Mode::Allocated),
        &size::Links::default(),
        &cancel,
    )
    .unwrap();
    assert!(r.cancelled && !r.complete());
}

#[cfg(unix)]
#[test]
fn unreadable_folder_is_a_permission_only_partial() {
    use std::os::unix::fs::PermissionsExt;
    let t = TempDir::new();
    write(&t.path().join("ok/f"), 1 << 20);
    write(&t.path().join("locked/f"), 1 << 20);
    let locked = t.path().join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let r = size::measure(t.path(), &opts(Mode::Allocated));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r = r.unwrap();
    assert!(r.errors > 0 && r.permission_only && !r.complete());
    assert!(r.bytes >= 1 << 20 && r.bytes < 2 << 20);
}

/// `Allocated` + one volume must equal `du -skPx`, to the KiB.
#[cfg(target_os = "macos")]
#[test]
fn allocated_matches_du() {
    let t = TempDir::new();
    tree(t.path());
    std::fs::hard_link(t.path().join("big.bin"), t.path().join("a/big-link")).unwrap();
    std::os::unix::fs::symlink(t.path().join("big.bin"), t.path().join("link")).unwrap();
    let clone = std::process::Command::new("/bin/cp")
        .arg("-c")
        .arg(t.path().join("big.bin"))
        .arg(t.path().join("clone.bin"))
        .status();
    assert!(clone.unwrap().success());
    let mut paths = vec![
        t.path().to_path_buf(),
        t.path().join("a"),
        t.path().join("big.bin"),
        t.path().join("link"),
    ];
    if let Some(extra) = std::env::var_os("OC_EXTRA_DIRS") {
        paths.extend(std::env::split_paths(&extra));
    }
    for p in paths {
        let out = std::process::Command::new("/usr/bin/du")
            .arg("-skPx")
            .arg(&p)
            .output()
            .unwrap();
        let du: u64 = String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let ours = size::measure(&p, &opts(Mode::Allocated)).unwrap();
        assert_eq!(size::kib(ours.bytes), du, "{}", p.display());
    }
}

/// An APFS clone shares its blocks with the original: deleting it frees (almost) nothing.
#[cfg(target_os = "macos")]
#[test]
fn freed_excludes_clone_blocks_shared_with_files_outside() {
    let t = TempDir::new();
    write(&t.path().join("orig"), 4 << 20);
    std::fs::create_dir(t.path().join("d")).unwrap();
    let ok = std::process::Command::new("/bin/cp")
        .arg("-c")
        .arg(t.path().join("orig"))
        .arg(t.path().join("d/clone"))
        .status()
        .unwrap();
    assert!(ok.success());
    write(&t.path().join("d/own"), 1 << 20);
    let freed = size::measure(&t.path().join("d"), &opts(Mode::Freed))
        .unwrap()
        .bytes;
    let alloc = size::measure(&t.path().join("d"), &opts(Mode::Allocated))
        .unwrap()
        .bytes;
    assert!(alloc >= 5 << 20, "{alloc}");
    assert!((1 << 20..2 << 20).contains(&freed), "{freed}");
}

/// A cancelled listing stops between bulk batches and says it is partial.
#[cfg(any(target_os = "macos", windows))]
#[test]
fn cancelled_listing_stops_between_batches() {
    let t = TempDir::new();
    tree(t.path());
    let mut seen = 0;
    let cancel = AtomicBool::new(true);
    let err =
        fs::read_dir_cancellable(t.path(), Detail::Basic, &cancel, &mut |_| seen += 1).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(seen, 0);
}

/// Many workers on a small tree, many times: every walk ends (no worker sleeps forever)
/// and sees every entry, whether a directory adds subdirectories or none.
#[test]
fn walks_with_many_idle_workers_finish() {
    use oc_engine::fs::Entry;
    use oc_engine::fs::walk::{self, Visitor};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Count(AtomicUsize);
    impl Visitor for Count {
        fn entry(&self, _: &Path, tag: u32, e: &Entry) -> Option<u32> {
            self.0.fetch_add(1, Ordering::Relaxed);
            (e.kind == Kind::Dir).then_some(tag)
        }
        fn error(&self, _: &Path, _: u32, _: std::io::Error) {}
    }
    let t = TempDir::new();
    tree(t.path());
    for threads in [1, 2, 16, 64] {
        for _ in 0..50 {
            let c = Count(AtomicUsize::new(0));
            walk::walk(
                t.path(),
                threads,
                Detail::Basic,
                &AtomicBool::new(false),
                &c,
            );
            // a + a/0..6, 40 files; big.bin; deep/x/y/z/w + f.bin
            assert_eq!(c.0.load(Ordering::Relaxed), 8 + 40 + 1 + 6);
        }
    }
}
