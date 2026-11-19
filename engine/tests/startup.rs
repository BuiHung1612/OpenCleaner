//! Startup Items: reading launchd plists and TCC grants, and that only a plain plist
//! directly in the user's own LaunchAgents may be turned off. Nothing here calls
//! launchctl: every disable below is refused before it would.
#![cfg(target_os = "macos")]

mod common;

use common::{TempDir, write};
use oc_engine::context::Context;
use oc_engine::startup;
use std::collections::HashMap;
use std::path::Path;

fn home() -> (TempDir, Context) {
    let dir = TempDir::new();
    std::fs::create_dir_all(dir.path().join("home/Library/LaunchAgents")).unwrap();
    let ctx = Context {
        home: dir.path().join("home"),
        vars: HashMap::new(),
        data_dir: dir.path().join("data"),
    };
    (dir, ctx)
}

fn plist(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>{body}</dict></plist>"#
        ),
    )
    .unwrap();
}

#[test]
fn plists_give_label_program_and_state() {
    let (_t, ctx) = home();
    let p = startup::agents_dir(&ctx).join("com.example.helper.plist");
    plist(
        &p,
        "<key>Label</key><string>com.example.helper</string>\
         <key>ProgramArguments</key><array>\
         <string>/Applications/Example.app/Contents/MacOS/helper</string><string>--quiet</string>\
         </array><key>Disabled</key><true/>",
    );
    let item = startup::read_plist(&p, "user").unwrap();
    assert_eq!(item.label, "com.example.helper");
    assert_eq!(
        item.program,
        "/Applications/Example.app/Contents/MacOS/helper --quiet"
    );
    assert_eq!(item.name, "Example", "named after the app it lives in");
    assert!(!item.enabled);

    let q = startup::agents_dir(&ctx).join("tool.plist");
    plist(
        &q,
        "<key>Label</key><string>tool</string><key>Program</key><string>/usr/local/bin/tool</string>",
    );
    let item = startup::read_plist(&q, "user").unwrap();
    assert_eq!((item.name.as_str(), item.enabled), ("tool", true));

    std::fs::write(
        startup::agents_dir(&ctx).join("broken.plist"),
        "not a plist",
    )
    .unwrap();
    write(&startup::agents_dir(&ctx).join("notes.txt"), 1);
    let user: Vec<_> = startup::list(&ctx)
        .into_iter()
        .filter(|i| i.scope == "user")
        .collect();
    assert_eq!(
        user.len(),
        2,
        "unreadable plists and other files are not listed"
    );
}

#[test]
fn only_plain_plists_directly_in_the_users_launch_agents_may_be_turned_off() {
    let (t, ctx) = home();
    let dir = startup::agents_dir(&ctx);
    let ok = dir.join("a.plist");
    plist(&ok, "<key>Label</key><string>a</string>");
    assert!(startup::check_user_file(&ctx, &ok, &dir).is_ok());

    let nested = dir.join("sub/b.plist");
    plist(&nested, "<key>Label</key><string>b</string>");
    let elsewhere = ctx.home.join("Library/LaunchDaemons/c.plist");
    plist(&elsewhere, "<key>Label</key><string>c</string>");
    let outside = t.path().join("outside/d.plist");
    plist(&outside, "<key>Label</key><string>d</string>");
    let link = dir.join("link.plist");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    for refused in [
        nested,
        elsewhere,
        outside.clone(),
        link,
        dir.join("../LaunchAgents/a.plist"),
        Path::new("/Library/LaunchAgents/x.plist").to_path_buf(),
    ] {
        assert!(
            startup::disable(&ctx, &refused, "").is_err(),
            "{} must be refused",
            refused.display()
        );
    }
    assert!(outside.exists() && ok.exists());

    // LaunchAgents itself swapped for a link to elsewhere
    let (t2, ctx2) = home();
    let real = t2.path().join("elsewhere");
    plist(&real.join("e.plist"), "<key>Label</key><string>e</string>");
    let agents = startup::agents_dir(&ctx2);
    std::fs::remove_dir(&agents).unwrap();
    std::os::unix::fs::symlink(&real, &agents).unwrap();
    assert!(startup::disable(&ctx2, &agents.join("e.plist"), "").is_err());
    assert!(real.join("e.plist").exists());
}

#[test]
fn tcc_grants_are_read_and_a_missing_database_has_none() {
    let t = TempDir::new();
    assert!(
        startup::read_tcc(&t.path().join("none.db"))
            .unwrap()
            .is_empty()
    );
    let db = t.path().join("TCC.db");
    let status = std::process::Command::new("/usr/bin/sqlite3")
        .arg(&db)
        .arg(
            "CREATE TABLE access (service TEXT, client TEXT, auth_value INTEGER);\
             INSERT INTO access VALUES ('kTCCServiceCamera', 'us.zoom.xos', 2);\
             INSERT INTO access VALUES ('kTCCServiceMicrophone', 'com.denied', 0);\
             INSERT INTO access VALUES ('kTCCServiceAddressBook', 'com.other', 2);",
        )
        .status()
        .unwrap();
    assert!(status.success());
    let grants = startup::read_tcc(&db).unwrap();
    assert_eq!(grants.len(), 1, "{grants:?}");
    assert_eq!(grants[0]["service"], "camera");
    assert_eq!(grants[0]["client"], "us.zoom.xos");
}

/// Records `path` as turned off by OpenCleaner, its plist at `trashed`.
fn turned_off(ctx: &Context, path: &Path, label: &str, trashed: Option<&Path>) {
    use oc_engine::ledger::{Line, Writer};
    Writer::open(ctx).write(&Line::Startup {
        session: 1,
        path: path.to_path_buf(),
        label: label.into(),
        program: "/Applications/Example.app/Contents/MacOS/helper".into(),
        trashed: trashed.map(Path::to_path_buf),
        reg_type: None,
    });
}

#[test]
fn a_plist_turned_off_by_opencleaner_is_put_back_from_the_trash() {
    let (_t, ctx) = home();
    let dir = startup::agents_dir(&ctx);
    let trash = ctx.home.join(".Trash");
    let original = dir.join("com.example.helper.plist");
    let in_trash = trash.join("com.example.helper 2.plist");
    plist(
        &in_trash,
        "<key>Label</key><string>com.example.helper</string>",
    );
    turned_off(&ctx, &original, "com.example.helper", Some(&in_trash));

    let listed = startup::turned_off(&ctx);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "Example");
    assert_eq!(listed[0].path, original);

    // something else took its place: refused, nothing moved
    plist(&original, "<key>Label</key><string>other</string>");
    assert!(startup::put_back(&ctx, &original).is_err());
    assert!(startup::turned_off(&ctx).is_empty());
    assert!(in_trash.exists());
    std::fs::remove_file(&original).unwrap();

    startup::put_back(&ctx, &original).unwrap();
    assert!(original.exists() && !in_trash.exists());
    assert!(startup::turned_off(&ctx).is_empty());
    assert!(
        startup::put_back(&ctx, &original).is_err(),
        "not in the Trash any more"
    );

    // never turned off by OpenCleaner
    let stranger = dir.join("x.plist");
    plist(&trash.join("x.plist"), "<key>Label</key><string>x</string>");
    assert!(startup::put_back(&ctx, &stranger).is_err());
    // a record pointing out of the Trash, or out of LaunchAgents
    let loose = ctx.home.join("loose.plist");
    plist(&loose, "<key>Label</key><string>y</string>");
    turned_off(&ctx, &dir.join("y.plist"), "y", Some(&loose));
    assert!(startup::put_back(&ctx, &dir.join("y.plist")).is_err());
    assert!(loose.exists());
    let elsewhere = ctx.home.join("Library/z.plist");
    plist(&trash.join("z.plist"), "<key>Label</key><string>z</string>");
    turned_off(&ctx, &elsewhere, "z", Some(&trash.join("z.plist")));
    assert!(startup::put_back(&ctx, &elsewhere).is_err());
    assert!(!elsewhere.exists());

    // older records without the Trash path: the same name in the Trash, same label only
    let old = dir.join("old.plist");
    plist(
        &trash.join("old.plist"),
        "<key>Label</key><string>old</string>",
    );
    turned_off(&ctx, &old, "old", None);
    let other = dir.join("other.plist");
    plist(
        &trash.join("other.plist"),
        "<key>Label</key><string>not-it</string>",
    );
    turned_off(&ctx, &other, "other", None);
    startup::put_back(&ctx, &old).unwrap();
    assert!(startup::put_back(&ctx, &other).is_err());
}
