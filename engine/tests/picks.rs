//! Uninstaller, Installers and Project Junk: what each may touch, what it finds, and that
//! removal still goes through the guards. Each test has its own home.

mod common;

use common::{TempDir, write};
use oc_engine::apps;
use oc_engine::context::Context;
use oc_engine::finders;
use oc_engine::guard::{Purpose, check_for};
use oc_engine::picks;
use std::collections::HashMap;
use std::path::PathBuf;
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
    set_mtime(path, SystemTime::now() - Duration::from_secs(days * 86_400));
}

/// Sets a file's or folder's modification time (Windows opens folders only with
/// FILE_FLAG_BACKUP_SEMANTICS, and needs write access to change times).
fn set_mtime(path: &std::path::Path, t: SystemTime) {
    let mut options = std::fs::File::options();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.write(true).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    options.read(true);
    options.open(path).unwrap().set_modified(t).unwrap();
}

#[test]
fn each_purpose_opens_only_what_it_needs() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Library/Preferences/com.x.plist"), 10);
    write(&h("Downloads/setup.dmg"), 10);
    write(&h("Downloads/thesis.pdf"), 10);
    write(&h("Documents/proj/node_modules/a"), 10);
    write(&h("Documents/proj/src/main.js"), 10);

    // macOS keeps preferences in ~/Library/Preferences: only the Uninstaller may touch them
    #[cfg(target_os = "macos")]
    {
        let prefs = h("Library/Preferences/com.x.plist");
        assert!(check_for(&prefs, &h("Library/Preferences"), &ctx, Purpose::Clean).is_err());
        assert!(check_for(&prefs, &h("Library/Preferences"), &ctx, Purpose::Uninstall).is_ok());
    }

    let dmg = h("Downloads/setup.dmg");
    assert!(check_for(&dmg, &h("Downloads"), &ctx, Purpose::Clean).is_err());
    assert!(check_for(&dmg, &h("Downloads"), &ctx, Purpose::Installers).is_ok());
    assert!(
        check_for(
            &h("Downloads/thesis.pdf"),
            &h("Downloads"),
            &ctx,
            Purpose::Installers
        )
        .is_err()
    );

    let nm = h("Documents/proj/node_modules");
    assert!(check_for(&nm, &h("Documents"), &ctx, Purpose::Projects).is_ok());
    assert!(check_for(&nm, &h("Documents"), &ctx, Purpose::Clean).is_err());
    assert!(
        check_for(
            &h("Documents/proj/src"),
            &h("Documents"),
            &ctx,
            Purpose::Projects
        )
        .is_err()
    );
    assert!(
        check_for(
            &h("Documents/proj"),
            &h("Documents"),
            &ctx,
            Purpose::Projects
        )
        .is_err()
    );

    // never outside the home, whatever the purpose (apps in /Applications aside)
    for p in [
        Purpose::Clean,
        Purpose::Uninstall,
        Purpose::Installers,
        Purpose::Projects,
    ] {
        assert!(
            check_for(
                std::path::Path::new("/etc/hosts"),
                std::path::Path::new("/etc"),
                &ctx,
                p
            )
            .is_err()
        );
    }
    assert!(
        check_for(
            std::path::Path::new("/Applications/Utilities/x.dmg"),
            std::path::Path::new("/Applications"),
            &ctx,
            Purpose::Uninstall
        )
        .is_err()
    );
}

/// macOS app leftovers (Windows apps are removed by their own uninstallers).
#[cfg(target_os = "macos")]
#[test]
fn leftovers_match_the_bundle_id_exactly_and_names_only_when_distinctive() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    for p in [
        "Library/Application Support/com.acme.Editor/state",
        "Library/Caches/com.acme.Editor/blob",
        "Library/Preferences/com.acme.Editor.plist",
        "Library/Preferences/com.acme.Editor.helper.plist",
        "Library/Containers/com.acme.Editor/Data/x",
        "Library/Group Containers/ABCDE12345.com.acme.Editor/x",
        "Library/Application Support/AcmeEditor/settings.json",
        // look-alikes that belong to something else
        "Library/Preferences/com.acme.EditorPro.plist",
        "Library/Application Support/com.acme.Editor2/x",
        "Library/Caches/Go/x",
    ] {
        write(&h(p), 1000);
    }
    let app = apps::App {
        name: "AcmeEditor".into(),
        bundle_id: "com.acme.Editor".into(),
        version: "1".into(),
        path: h("Applications/AcmeEditor.app"),
        running: false,
        ..Default::default()
    };
    std::fs::create_dir_all(&app.path).unwrap();
    let picks = apps::uninstall_picks(&app, &ctx, &AtomicBool::new(false));
    let found: Vec<String> = picks
        .iter()
        .map(|p| {
            p.path
                .strip_prefix(&ctx.home)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    for want in [
        "Applications/AcmeEditor.app",
        "Library/Application Support/com.acme.Editor",
        "Library/Caches/com.acme.Editor",
        "Library/Preferences/com.acme.Editor.plist",
        "Library/Preferences/com.acme.Editor.helper.plist",
        "Library/Containers/com.acme.Editor",
        "Library/Group Containers/ABCDE12345.com.acme.Editor",
        "Library/Application Support/AcmeEditor",
    ] {
        assert!(found.iter().any(|f| f == want), "missing {want}: {found:?}");
    }
    for not in ["EditorPro", "Editor2", "Caches/Go"] {
        assert!(
            !found.iter().any(|f| f.contains(not)),
            "matched a look-alike {not}: {found:?}"
        );
    }

    // a two-letter name is not used to guess folders
    let short = apps::App {
        name: "Go".into(),
        bundle_id: "org.example.go".into(),
        path: app.path.clone(),
        ..Default::default()
    };
    let picks = apps::uninstall_picks(&short, &ctx, &AtomicBool::new(false));
    assert!(!picks.iter().any(|p| p.path.ends_with("Go")));
}

#[test]
fn installers_are_listed_by_extension_and_old_ones_preselected() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Downloads/Old.dmg"), 5000);
    age(&h("Downloads/Old.dmg"), 30);
    write(&h("Downloads/New.pkg"), 5000);
    write(&h("Downloads/sub/Nested.iso"), 5000);
    write(&h("Downloads/sub/deeper/TooDeep.dmg"), 5000);
    write(&h("Downloads/report.pdf"), 5000);
    write(&h("Documents/Kept.dmg"), 5000);
    let found = finders::installers(&ctx);
    let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(sorted, ["Nested.iso", "New.pkg", "Old.dmg"]);
    assert!(
        found
            .iter()
            .find(|p| p.name == "Old.dmg")
            .unwrap()
            .preselect
    );
    assert!(
        !found
            .iter()
            .find(|p| p.name == "New.pkg")
            .unwrap()
            .preselect
    );
}

#[test]
fn project_junk_needs_its_project_file_and_skips_tracked_and_marks_active() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    // a JS project, idle for a month
    write(&h("Projects/web/package.json"), 10);
    write(&h("Projects/web/node_modules/lib/index.js"), 200_000);
    for p in [
        "Projects/web/package.json",
        "Projects/web/node_modules",
        "Projects/web",
    ] {
        age(&h(p), 30);
    }
    // a "target" folder without Cargo.toml is somebody's data
    write(&h("Projects/notes/target/plan.txt"), 50_000);
    // an active Rust project
    write(&h("Projects/rusty/Cargo.toml"), 10);
    write(&h("Projects/rusty/target/debug/app"), 100_000);
    // a venv only counts with its pyvenv.cfg
    write(&h("Projects/py/.venv/pyvenv.cfg"), 10);
    write(&h("Projects/py/.venv/lib/x.py"), 30_000);

    let found = finders::projects(&ctx, &[], &AtomicBool::new(false));
    let rel: Vec<String> = found
        .iter()
        .map(|p| {
            p.path
                .strip_prefix(&ctx.home)
                .unwrap()
                .display()
                .to_string()
        })
        .collect();
    assert!(
        rel.contains(&"Projects/web/node_modules".to_string()),
        "{rel:?}"
    );
    assert!(
        rel.contains(&"Projects/rusty/target".to_string()),
        "{rel:?}"
    );
    assert!(rel.contains(&"Projects/py/.venv".to_string()), "{rel:?}");
    assert!(!rel.iter().any(|r| r.contains("notes")), "{rel:?}");
    let pick = |r: &str| found.iter().find(|p| p.path.ends_with(r)).unwrap();
    assert!(pick("web/node_modules").preselect);
    assert!(
        !pick("rusty/target").preselect,
        "an active project is listed but not preselected"
    );

    // removal goes through the guards and deletes only the picked folder
    let list = picks::store("projects", found, &ctx);
    let idx = list
        .items
        .iter()
        .position(|p| p.path.ends_with("web/node_modules"))
        .unwrap();
    let out = picks::execute(
        &ctx,
        list.plan,
        &[idx],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!(out.removed, 1);
    assert!(!h("Projects/web/node_modules").exists());
    assert!(
        h("Projects/web/package.json").exists() && h("Projects/rusty/target/debug/app").exists()
    );
}

#[test]
fn folders_with_tracked_files_are_not_junk() {
    let git = [
        "/opt/homebrew/bin/git",
        "/usr/local/bin/git",
        "/usr/bin/git",
    ]
    .into_iter()
    .find(|g| std::path::Path::new(g).exists());
    let Some(git) = git else { return };
    if std::process::Command::new(git)
        .arg("--version")
        .output()
        .map(|o| !o.status.success())
        .unwrap_or(true)
    {
        return;
    }
    let (_t, ctx) = home();
    let repo = ctx.home.join("Projects/app");
    write(&repo.join("build.gradle"), 10);
    write(&repo.join("build/committed.txt"), 50_000);
    let run = |args: &[&str]| {
        std::process::Command::new(git)
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
    };
    run(&["init", "-q"]);
    run(&["add", "build/committed.txt", "build.gradle"]);
    let found = finders::projects(&ctx, &[], &AtomicBool::new(false));
    assert!(
        found.iter().all(|p| !p.path.ends_with("build")),
        "a folder with tracked files was listed"
    );
}

#[test]
fn a_pick_changed_after_listing_is_left_alone() {
    let (_t, ctx) = home();
    let h = |p: &str| ctx.home.join(p);
    write(&h("Projects/web/package.json"), 10);
    write(&h("Projects/web/node_modules/x"), 10_000);
    let list = picks::store(
        "projects",
        finders::projects(&ctx, &[], &AtomicBool::new(false)),
        &ctx,
    );
    let idx = list
        .items
        .iter()
        .position(|p| p.path.ends_with("node_modules"))
        .unwrap();
    std::fs::remove_dir_all(h("Projects/web/node_modules")).unwrap();
    write(&h("Projects/web/node_modules/new-and-precious"), 10);
    let out = picks::execute(
        &ctx,
        list.plan,
        &[idx],
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap();
    assert_eq!((out.removed, out.skipped), (0, 1));
    assert!(h("Projects/web/node_modules/new-and-precious").exists());
    let _: PathBuf = h("x");
}

#[test]
fn app_sizes_are_remembered_until_the_app_changes() {
    let (_t, ctx) = home();
    let app = ctx.home.join("Applications/Thing.app");
    write(&app.join("Contents/Info.plist"), 100);
    write(&app.join("Contents/MacOS/thing"), 100_000);
    let other = ctx.home.join("Applications/Other.app");
    write(&other.join("Contents/Info.plist"), 100);
    write(&other.join("Contents/MacOS/other"), 50_000);
    let paths = vec![app.clone(), other.clone()];
    let first = apps::sizes(&ctx, &paths, &AtomicBool::new(false), &mut |_| {});
    assert!(first[0] >= 100_000 && first[1] >= 50_000);

    // remembered: grows on disk without a new stamp, the old size is reported
    write(&app.join("Contents/Resources/big"), 1_000_000);
    std::fs::File::options()
        .write(true)
        .open(app.join("Contents"))
        .ok();
    let t = SystemTime::now() - Duration::from_secs(3600);
    for p in [
        app.clone(),
        app.join("Contents"),
        app.join("Contents/Info.plist"),
    ] {
        set_mtime(&p, t);
    }
    let stamped = apps::sizes(&ctx, &paths, &AtomicBool::new(false), &mut |_| {});
    let again = apps::sizes(&ctx, &paths, &AtomicBool::new(false), &mut |_| {});
    assert_eq!(stamped, again, "an unchanged app is not walked again");

    // an update touches Info.plist: measured again
    write(&app.join("Contents/Info.plist"), 120);
    let updated = apps::sizes(&ctx, &paths, &AtomicBool::new(false), &mut |_| {});
    assert!(updated[0] >= 1_100_000, "{updated:?}");
    assert_eq!(updated[1], first[1]);
}
