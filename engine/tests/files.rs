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
    // structured detail, for apps that word it themselves
    assert_eq!(shot.kind, "screenshot");
    assert!(shot.age_days.unwrap() >= 30);
    let mov = found.iter().find(|p| p.name == "old.mov").unwrap();
    assert!(!mov.preselect);
    assert!(mov.idle_days.unwrap() >= 180);
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
    assert_eq!(kept.flags, ["kept"]);
    assert!(
        found
            .iter()
            .all(|p| p.copies == Some(2) && !p.place.is_empty())
    );
    assert!(
        found
            .iter()
            .any(|p| p.preselect && p.path.ends_with("copy of a.zip"))
    );
}

/// Never reaches the Trash: every case here must be refused before anything moves.
#[test]
fn removing_duplicates_always_leaves_one_unchanged_copy() {
    use oc_engine::picks;
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Downloads/a.zip"), 2 * MIB);
    age(&h("Downloads/a.zip"), 10);
    write(&h("Documents/b.zip"), 2 * MIB);
    let run = |items: &[usize], plan: u64| {
        picks::execute(
            &ctx,
            plan,
            items,
            &HashMap::new(),
            &AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap()
    };
    let list = picks::store(
        "duplicates",
        files::duplicates(&ctx, 1, &AtomicBool::new(false)),
        &ctx,
    );
    assert_eq!(list.items.len(), 2);
    let kept = list.items.iter().position(|p| !p.preselect).unwrap();
    let copy = 1 - kept;

    // every copy of the set picked: none goes
    let out = run(&[0, 1], list.plan);
    assert_eq!((out.removed, out.skipped), (0, 2));
    assert!(
        out.results
            .iter()
            .all(|r| r == "skipped: no copy would be left")
    );

    // the copy that stays was edited since (same file, new length): the set is left alone
    std::fs::OpenOptions::new()
        .write(true) // set_len needs write access (append is not enough on Windows)
        .open(&list.items[kept].path)
        .unwrap()
        .set_len(2 * MIB as u64 + 1)
        .unwrap();
    let out = run(&[copy], list.plan);
    assert_eq!(out.results, ["skipped: no copy would be left"]);
    assert!(h("Downloads/a.zip").exists() && h("Documents/b.zip").exists());
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

/// Two copies and a same-size, same-head impostor, all a few days old (files changed in
/// the last seconds are never cached).
fn dup_home() -> (TempDir, Context) {
    let (t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Downloads/a.zip"), 2 * MIB);
    write(&h("Documents/copy of a.zip"), 2 * MIB);
    let mut other = vec![7u8; 2 * MIB];
    *other.last_mut().unwrap() = 8;
    std::fs::create_dir_all(h("Desktop")).unwrap();
    std::fs::write(h("Desktop/other.zip"), &other).unwrap();
    for p in [
        "Downloads/a.zip",
        "Documents/copy of a.zip",
        "Desktop/other.zip",
    ] {
        age(&h(p), 3);
    }
    (t, ctx)
}

/// Runs `duplicates`; returns the names found and how many files were read to hash.
fn dups(ctx: &Context) -> (Vec<String>, usize) {
    let before = files::files_hashed();
    let found = files::duplicates(ctx, 1, &AtomicBool::new(false));
    let mut names: Vec<String> = found.iter().map(|p| p.name.clone()).collect();
    names.sort();
    (names, files::files_hashed() - before)
}

#[test]
fn duplicates_reuse_cached_hashes_and_rehash_changed_files() {
    let (_t, ctx) = dup_home();
    let want = vec!["a.zip".to_string(), "copy of a.zip".to_string()];
    // first run: head hash of 3 files, full hash of 3 (other.zip shares the head)
    assert_eq!(dups(&ctx), (want.clone(), 6));
    // unchanged: everything from the cache
    assert_eq!(dups(&ctx), (want.clone(), 0));
    // a changed mtime is a changed file: its two hashes are read again
    age(&ctx.home.join("Documents/copy of a.zip"), 5);
    assert_eq!(dups(&ctx), (want.clone(), 2));
    assert_eq!(dups(&ctx), (want.clone(), 0));
    // a damaged tail is cut off, the lines before it still count
    let cache = ctx.data_dir.join("dup-hashes-v1.jsonl");
    let mut text = std::fs::read_to_string(&cache).unwrap();
    let last = text.trim_end().rfind('\n').unwrap() + 1;
    text.truncate(last + 5);
    std::fs::write(&cache, text).unwrap();
    assert_eq!(dups(&ctx), (want.clone(), 2));
    assert_eq!(dups(&ctx), (want, 0));
}

#[test]
fn a_forged_hash_cache_never_makes_a_different_file_a_duplicate() {
    let (_t, ctx) = dup_home();
    files::duplicates(&ctx, 1, &AtomicBool::new(false));
    // every cached file now claims the same head and full hash
    let cache = ctx.data_dir.join("dup-hashes-v1.jsonl");
    let text = std::fs::read_to_string(&cache).unwrap();
    let mut lines = text.lines();
    let mut forged = format!("{}\n", lines.next().unwrap());
    for line in lines {
        let mut v: Vec<serde_json::Value> = serde_json::from_str(line).unwrap();
        v[4] = 1.into();
        v[5] = 1.into();
        forged.push_str(&serde_json::to_string(&v).unwrap());
        forged.push('\n');
    }
    assert_eq!(forged.lines().count(), 4);
    std::fs::write(&cache, forged).unwrap();
    let (names, hashed) = dups(&ctx);
    assert_eq!(hashed, 0, "the forged entries were used");
    // the byte-for-byte comparison still keeps other.zip out
    assert_eq!(names, ["a.zip", "copy of a.zip"]);
}

#[test]
fn huge_limits_saturate_instead_of_overflowing() {
    let (_t, ctx) = home();
    write(&ctx.home.join("Movies/old.mov"), 2 * MIB);
    age(&ctx.home.join("Movies/old.mov"), 400);
    let no = AtomicBool::new(false);
    assert!(files::large_old(&ctx, u64::MAX, 1, &no).is_empty());
    assert!(files::large_old(&ctx, 1, i64::MAX, &no).is_empty());
    assert_eq!(files::large_old(&ctx, 1, i64::MIN, &no).len(), 1);
    assert!(files::duplicates(&ctx, u64::MAX, &no).is_empty());
}
