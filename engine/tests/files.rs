//! Large & old files and duplicates: only the user's plain files, never inside packages.

mod common;

use common::{TempDir, write};
use oc_engine::context::Context;
use oc_engine::files;
use oc_engine::guard::{Purpose, check_for};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime};

fn home() -> (TempDir, Context) {
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    let ctx = Context {
        home: dir.path().join("home"),
        vars: HashMap::new(),
        data_dir: dir.path().join("data"),
    };
    (dir, ctx)
}

fn age(path: &std::path::Path, days: u64) {
    let t = SystemTime::now() - Duration::from_secs(days * 86_400);
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(t).set_accessed(t))
        .unwrap();
}

const MIB: usize = 1 << 20;

#[test]
fn large_old_files_and_old_screenshots_but_nothing_inside_packages() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Movies/old.mov"), 2 * MIB);
    age(&h("Movies/old.mov"), 400);
    write(&h("Movies/new.mov"), 2 * MIB);
    write(
        &h("Pictures/Photos Library.photoslibrary/originals/big.heic"),
        2 * MIB,
    );
    age(
        &h("Pictures/Photos Library.photoslibrary/originals/big.heic"),
        400,
    );
    write(&h("Desktop/Screenshot 2026-01-01 at 10.00.00.png"), 1000);
    age(&h("Desktop/Screenshot 2026-01-01 at 10.00.00.png"), 60);
    write(&h("Desktop/Screenshot fresh.png"), 1000);

    let found = files::large_old(&ctx, 1, 180, &AtomicBool::new(false));
    let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"old.mov"), "{names:?}");
    assert!(
        names.contains(&"Screenshot 2026-01-01 at 10.00.00.png"),
        "{names:?}"
    );
    assert_eq!(names.len(), 2, "{names:?}");
    let shot = found
        .iter()
        .find(|p| p.name.starts_with("Screenshot"))
        .unwrap();
    assert!(shot.preselect);
    assert!(
        !found
            .iter()
            .find(|p| p.name == "old.mov")
            .unwrap()
            .preselect
    );
}

#[test]
fn duplicates_are_byte_for_byte_and_keep_the_oldest() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Downloads/a.zip"), 2 * MIB);
    age(&h("Downloads/a.zip"), 10);
    write(&h("Documents/copy of a.zip"), 2 * MIB);
    // same size, same first 64 KiB, different end: not a copy
    let mut other = vec![7u8; 2 * MIB];
    *other.last_mut().unwrap() = 8;
    std::fs::create_dir_all(h("Desktop")).unwrap();
    std::fs::write(h("Desktop/other.zip"), &other).unwrap();
    // a hard link is the same file, not a copy
    std::fs::create_dir_all(h("Music")).unwrap();
    std::fs::hard_link(h("Downloads/a.zip"), h("Music/link.zip")).unwrap();

    let found = files::duplicates(&ctx, 1, &AtomicBool::new(false));
    assert_eq!(
        found.len(),
        2,
        "{:?}",
        found.iter().map(|p| &p.path).collect::<Vec<_>>()
    );
    let kept = found.iter().find(|p| !p.preselect).unwrap();
    assert!(kept.path.ends_with("a.zip") || kept.path.ends_with("link.zip"));
    assert!(
        found
            .iter()
            .any(|p| p.preselect && p.path.ends_with("copy of a.zip"))
    );
}

#[test]
fn files_purpose_takes_plain_files_only() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Documents/f.bin"), 10);
    std::fs::create_dir_all(h("Documents/folder")).unwrap();
    assert!(check_for(&h("Documents/f.bin"), &h("Documents"), &ctx, Purpose::Files).is_ok());
    assert!(
        check_for(
            &h("Documents/folder"),
            &h("Documents"),
            &ctx,
            Purpose::Files
        )
        .is_err()
    );
    write(&h(".ssh/id"), 10);
    assert!(check_for(&h(".ssh/id"), &ctx.home, &ctx, Purpose::Files).is_err());
}
