mod common;

use common::{TempDir, write};
use oc_engine::space::{self, Listing};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn sizes(l: &Listing) -> Vec<(String, u64, u64)> {
    l.entries
        .iter()
        .map(|e| (e.name.clone(), e.size, e.files))
        .collect()
}

fn list(dir: &Path, cache: Option<&Path>) -> Listing {
    let l = space::list(dir, cache, &AtomicBool::new(false), &mut |_| {}).unwrap();
    assert_eq!(l.scan_status, "complete");
    l
}

/// The directory cache gives the same sizes as a full walk, sees entries added, and (the
/// documented trade-off) misses a file growing in place in an unchanged directory.
#[test]
fn dir_cache_reuses_unchanged_directories() {
    let t = TempDir::new();
    let root = t.path().join("tree");
    let cache = t.path().join("cache");
    // 40 files per folder
    for i in 0..200 {
        write(
            &root.join(format!("a/{}/{i}.bin", i % 5)),
            1000 + (i % 30) * 4096,
        );
    }
    write(&root.join("b/deep/x/y/f.bin"), 70_000);
    write(&root.join("top.bin"), 5000);
    // directories modified in the last seconds are not cached
    std::thread::sleep(std::time::Duration::from_secs(3));

    let full = list(&root, None);
    let first = list(&root, Some(&cache));
    assert_eq!(sizes(&first), sizes(&full));
    assert!(cache.join("space-dirs-v3.jsonl").is_file());

    // grows in place: a/3's mtime does not change, so the cached size stays
    std::fs::OpenOptions::new()
        .append(true)
        .open(root.join("a/3/3.bin"))
        .unwrap()
        .write_all(&vec![1u8; 1 << 20])
        .unwrap();
    assert_eq!(sizes(&list(&root, Some(&cache))), sizes(&full));
    assert_ne!(sizes(&list(&root, None)), sizes(&full));

    // an added file changes its directory's mtime: seen
    write(&root.join("b/deep/x/new.bin"), 200_000);
    let (cached, walked) = (sizes(&list(&root, Some(&cache))), sizes(&list(&root, None)));
    let b = |s: &[(String, u64, u64)]| s.iter().find(|e| e.0 == "b").cloned();
    assert_eq!(b(&cached), b(&walked));
    assert!(b(&cached).unwrap().1 > b(&sizes(&full)).unwrap().1);
}

/// Progress listings say which children are still being sized; the last one none.
#[test]
fn progress_marks_children_scanning_or_complete() {
    let t = TempDir::new();
    write(&t.path().join("f.bin"), 10);
    for i in 0..200 {
        write(&t.path().join(format!("d/{}/{i}", i % 20)), 10);
    }
    let mut seen = Vec::new();
    let l = space::list(t.path(), None, &AtomicBool::new(false), &mut |p| {
        seen.push(p.scan_status)
    })
    .unwrap();
    assert!(l.entries.iter().all(|e| e.scan_status == "complete"));
    assert!(seen.iter().all(|s| *s == "scanning" || *s == "complete"));
}
