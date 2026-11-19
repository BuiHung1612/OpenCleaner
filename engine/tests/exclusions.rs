//! Folders excluded in Settings, and putting trashed items back. Each test has its own home.

mod common;

use common::{TempDir, write};
use oc_engine::context::Context;
use oc_engine::guard::{self, Purpose, check_for};
use oc_engine::{files, trash};
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

#[test]
fn an_excluded_folder_is_refused_by_every_purpose() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    let kept = h("Downloads/keep");
    write(&kept.join("setup.dmg"), 10);
    write(&kept.join("node_modules/a"), 10);
    write(&h("Library/Caches/keep/x"), 10);
    assert!(
        check_for(
            &kept.join("setup.dmg"),
            &h("Downloads"),
            &ctx,
            Purpose::Installers
        )
        .is_ok()
    );

    guard::set_exclusions(&ctx, vec![kept.clone(), h("Library/Caches/keep")]).unwrap();
    assert_eq!(guard::exclusions(&ctx).len(), 2);
    for purpose in [
        Purpose::Clean,
        Purpose::Uninstall,
        Purpose::Installers,
        Purpose::Projects,
        Purpose::Files,
    ] {
        for item in [
            kept.join("setup.dmg"),
            kept.join("node_modules"),
            kept.clone(),
            h("Library/Caches/keep/x"),
        ] {
            assert_eq!(
                check_for(&item, &ctx.home, &ctx, purpose),
                Err("excluded in Settings"),
                "{purpose:?} {item:?}"
            );
        }
    }
    // a sibling is not excluded
    write(&h("Downloads/other.dmg"), 10);
    assert!(
        check_for(
            &h("Downloads/other.dmg"),
            &h("Downloads"),
            &ctx,
            Purpose::Installers
        )
        .is_ok()
    );

    // the list survives a restart (read back from the data folder)
    let saved: Vec<std::path::PathBuf> =
        serde_json::from_slice(&std::fs::read(ctx.data_dir.join("exclusions.json")).unwrap())
            .unwrap();
    assert_eq!(saved.len(), 2);
    guard::set_exclusions(&ctx, vec![]).unwrap();
    assert!(
        check_for(
            &kept.join("setup.dmg"),
            &h("Downloads"),
            &ctx,
            Purpose::Installers
        )
        .is_ok()
    );
}

#[test]
fn excluded_folders_are_not_scanned_for_large_files() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    for f in ["Movies/a.mov", "Movies/keep/b.mov"] {
        write(&h(f), 2 << 20);
        let t = SystemTime::now() - Duration::from_secs(400 * 86_400);
        let file = std::fs::File::options().write(true).open(h(f)).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(t).set_accessed(t))
            .unwrap();
    }
    guard::set_exclusions(&ctx, vec![h("Movies/keep")]).unwrap();
    let found = files::large_old(&ctx, 1, 180, &AtomicBool::new(false));
    let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["a.mov"]);
}

#[test]
fn put_back_moves_from_the_trash_only_to_a_free_place() {
    let dir = TempDir::new();
    let trashed = dir.path().join(".Trash/report.pdf");
    let original = dir.path().join("Documents/report.pdf");
    write(&trashed, 10);
    std::fs::create_dir_all(original.parent().unwrap()).unwrap();

    // something new took its place: never replaced
    write(&original, 3);
    assert!(trash::put_back(&original, &trashed).is_err());
    assert_eq!(std::fs::read(&original).unwrap().len(), 3);
    std::fs::remove_file(&original).unwrap();

    trash::put_back(&original, &trashed).unwrap();
    assert_eq!(std::fs::read(&original).unwrap().len(), 10);
    assert!(!trashed.exists());
    // already back
    assert!(trash::put_back(&original, &trashed).is_err());

    // only from a Trash folder
    let elsewhere = dir.path().join("Other/x");
    write(&elsewhere, 1);
    assert!(trash::put_back(&dir.path().join("Documents/x"), &elsewhere).is_err());
    assert!(elsewhere.exists());

    // a folder that is gone is not recreated
    write(&dir.path().join(".Trash/y"), 1);
    assert!(trash::put_back(&dir.path().join("Gone/y"), &dir.path().join(".Trash/y")).is_err());
}

#[test]
fn history_lists_trashed_items_still_in_the_trash_and_reads_old_lines() {
    use oc_engine::ledger;
    let (t, ctx) = home();
    let to = t.path().join(".Trash/a.dmg");
    write(&to, 1);
    std::fs::create_dir_all(&ctx.data_dir).unwrap();
    // a line from before Put Back (no `trashed`), then one with it, then the session
    let old = r#"{"t":"item","session":1,"rule":"installers","path":"/x/old.dmg","bytes":1,"result":"removed","reason":null}"#;
    std::fs::write(ledger::path(&ctx), format!("{old}\n")).unwrap();
    let mut w = ledger::Writer::open(&ctx);
    let from = ctx.home.join("Downloads/a.dmg");
    w.write(&ledger::Line::Item {
        session: 1,
        rule: "installers".into(),
        path: from.clone(),
        bytes: 1,
        result: "removed".into(),
        reason: None,
        trashed: Some(to.clone()),
    });
    w.write(&ledger::Line::Session {
        id: 1,
        command: "installers".into(),
        started: 1,
        finished: 1,
        items: 2,
        bytes: 2,
        failed: 0,
    });
    drop(w);
    let v = ledger::sessions(&ctx, 10);
    let trashed = v["sessions"][0]["trashed"].as_array().unwrap();
    assert_eq!(trashed.len(), 1);
    assert_eq!(trashed[0]["to"], to.to_string_lossy().as_ref());
    assert!(ledger::was_trashed(&ctx, &from, &to));
    assert!(!ledger::was_trashed(&ctx, &ctx.home.join("elsewhere"), &to));
}
