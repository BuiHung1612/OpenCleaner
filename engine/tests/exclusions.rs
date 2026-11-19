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
        stamp: guard::stamp(&to),
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
    assert!(ledger::trashed_stamp(&ctx, &from, &to).is_ok());
    assert!(ledger::trashed_stamp(&ctx, &ctx.home.join("elsewhere"), &to).is_err());
}

#[test]
fn put_back_moves_only_the_very_item_that_was_trashed() {
    use oc_engine::ledger;
    let (t, ctx) = home();
    let from = ctx.home.join("Downloads/a.dmg");
    let to = t.path().join(".Trash/a.dmg");
    std::fs::create_dir_all(from.parent().unwrap()).unwrap();
    write(&to, 10);
    std::fs::create_dir_all(&ctx.data_dir).unwrap();
    // an old line (no stamp) for another trashed item is never put back
    let old_to = t.path().join(".Trash/old.dmg");
    write(&old_to, 1);
    let old = format!(
        r#"{{"t":"item","session":1,"rule":"installers","path":{:?},"bytes":1,"result":"removed","reason":null,"trashed":{:?}}}"#,
        ctx.home.join("Downloads/old.dmg"),
        old_to
    );
    std::fs::write(ledger::path(&ctx), format!("{old}\n")).unwrap();
    let mut w = ledger::Writer::open(&ctx);
    w.write(&ledger::Line::Item {
        session: 2,
        rule: "installers".into(),
        path: from.clone(),
        bytes: 10,
        result: "removed".into(),
        reason: None,
        trashed: Some(to.clone()),
        stamp: guard::stamp(&to),
    });
    drop(w);
    assert!(
        ledger::trashed_stamp(&ctx, &ctx.home.join("Downloads/old.dmg"), &old_to).is_err(),
        "a line without a stamp is not put back"
    );

    // something else now sits in the Trash under that name: refused, left alone
    let was = ledger::trashed_stamp(&ctx, &from, &to).unwrap();
    std::fs::remove_file(&to).unwrap();
    write(&to, 99);
    assert!(trash::put_back_checked(&from, &to, &was).is_err());
    assert!(to.exists() && !from.exists());

    // the very item: put back
    let was = guard::stamp(&to).unwrap();
    trash::put_back_checked(&from, &to, &was).unwrap();
    assert_eq!(std::fs::read(&from).unwrap().len(), 99);
}

#[test]
fn an_item_holding_an_excluded_folder_is_refused_and_kept_by_the_scan() {
    use oc_engine::{clean, procs::Running, rules::RuleSet};
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Library/Caches/com.foo/keepme/precious"), 10);
    write(&h("Library/Caches/com.bar/x"), 10);
    write(&h("Projects/web/package.json"), 10);
    write(&h("Projects/web/node_modules/keep/x"), 10);
    guard::set_exclusions(
        &ctx,
        vec![
            h("Library/Caches/com.foo/keepme"),
            h("Projects/web/node_modules/keep"),
        ],
    )
    .unwrap();
    for (item, root, purpose) in [
        (
            h("Library/Caches/com.foo"),
            h("Library/Caches"),
            Purpose::Clean,
        ),
        (
            h("Library/Caches/com.foo"),
            h("Library/Caches"),
            Purpose::Uninstall,
        ),
        (
            h("Projects/web/node_modules"),
            h("Projects"),
            Purpose::Projects,
        ),
    ] {
        assert_eq!(
            check_for(&item, &root, &ctx, purpose),
            Err("holds a folder excluded in Settings"),
            "{item:?}"
        );
    }
    assert!(
        check_for(
            &h("Library/Caches/com.bar"),
            &h("Library/Caches"),
            &ctx,
            Purpose::Clean
        )
        .is_ok()
    );
    // another spelling of the excluded folder (case; caught by name on macOS/Windows and
    // by identity anywhere the file system ignores case)
    if cfg!(any(target_os = "macos", windows)) {
        assert!(guard::excluded(
            &h("library/caches/COM.FOO/keepme/precious"),
            &ctx
        ));
    }

    let set = RuleSet::parse(
        "[[group]]\nid=\"g\"\ntitle=\"g\"\n[[rule]]\nid=\"caches\"\ngroup=\"g\"\ntitle=\"c\"\npaths=[\"~/Library/Caches\"]\n",
    )
    .unwrap();
    let plan = clean::scan_with(
        &ctx,
        &set,
        &Running::default(),
        true,
        &AtomicBool::new(false),
        &mut |_| {},
    );
    let listed: Vec<String> = plan
        .groups
        .iter()
        .flat_map(|g| {
            g.rules
                .iter()
                .flat_map(|r| r.paths.clone().unwrap_or_default())
        })
        .map(|p| format!("{p:?}"))
        .collect();
    assert!(listed.iter().any(|p| p.contains("com.bar")), "{listed:?}");
    assert!(!listed.iter().any(|p| p.contains("com.foo")), "{listed:?}");
}

#[test]
fn a_damaged_exclusions_file_refuses_every_removal_until_saved_again() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Library/Caches/com.bar/x"), 10);
    std::fs::create_dir_all(&ctx.data_dir).unwrap();
    std::fs::write(ctx.data_dir.join("exclusions.json"), b"[\"/half").unwrap();
    assert_eq!(
        check_for(
            &h("Library/Caches/com.bar"),
            &h("Library/Caches"),
            &ctx,
            Purpose::Clean
        ),
        Err(guard::UNREADABLE)
    );
    assert!(guard::load_exclusions(&ctx).is_err());
    // saving replaces it (atomically: no temporary file left behind)
    guard::set_exclusions(&ctx, vec![]).unwrap();
    assert!(
        check_for(
            &h("Library/Caches/com.bar"),
            &h("Library/Caches"),
            &ctx,
            Purpose::Clean
        )
        .is_ok()
    );
    let names: Vec<_> = std::fs::read_dir(&ctx.data_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["exclusions.json"]);
}

#[test]
fn space_lens_resolves_the_path_before_checking_exclusions() {
    let (t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Work/keep/a.txt"), 10);
    write(&h("Work/other.txt"), 10);
    guard::set_exclusions(&ctx, vec![h("Work/keep")]).unwrap();
    // `..`, a link to a parent, the folder itself or a folder holding it: all refused
    assert!(guard::check_trash_path(&h("Work/other/../keep/a.txt"), &ctx).is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(h("Work"), t.path().join("alias")).unwrap();
        assert_eq!(
            guard::check_trash_path(&t.path().join("alias/keep/a.txt"), &ctx),
            Err("excluded in Settings")
        );
        assert_eq!(
            guard::check_trash_path(&t.path().join("alias/keep"), &ctx),
            Err("excluded in Settings")
        );
    }
    assert_eq!(
        guard::check_trash_path(&h("Work"), &ctx),
        Err("holds a folder excluded in Settings")
    );
    if cfg!(any(target_os = "macos", windows)) {
        assert!(guard::check_trash_path(&h("work/KEEP/a.txt"), &ctx).is_err());
    }
    // a sibling is fine, and comes back resolved
    assert_eq!(
        guard::check_trash_path(&h("Work/other.txt"), &ctx),
        Ok(h("Work/other.txt"))
    );
    let _ = t;
}
