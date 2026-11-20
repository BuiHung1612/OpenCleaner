//! Safety suite for System Junk: every way an item could be the wrong thing to remove must
//! end with it left alone. Each test builds its own home in a temporary directory.

mod common;

use common::{TempDir, write};
use oc_engine::clean::{self, PlanSummary};
use oc_engine::context::Context;
use oc_engine::procs::Running;
use oc_engine::rules::RuleSet;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;

struct Home {
    #[allow(dead_code)] // keeps the temporary directory alive
    dir: TempDir,
    ctx: Context,
}

impl Home {
    fn new() -> Home {
        let dir = TempDir::new();
        std::fs::create_dir_all(dir.path().join("home")).unwrap();
        let ctx = Context {
            home: dir.path().join("home"),
            vars: HashMap::new(),
            data_dir: dir.path().join("data"),
        };
        Home { dir, ctx }
    }
    fn p(&self, rel: &str) -> std::path::PathBuf {
        self.ctx.home.join(rel)
    }
    #[allow(dead_code)] // used by the unix-only tests
    fn outside(&self, rel: &str) -> std::path::PathBuf {
        self.dir.path().join("outside").join(rel)
    }
}

const GROUPS: &str = "[[group]]\nid = \"g\"\ntitle = \"G\"\n";

fn rules(body: &str) -> RuleSet {
    RuleSet::parse(&format!("{GROUPS}{body}")).unwrap()
}

fn scan(h: &Home, set: &RuleSet, running: &Running) -> PlanSummary {
    clean::scan(&h.ctx, set, running, &AtomicBool::new(false), &mut |_| {})
}

fn rule<'a>(plan: &'a PlanSummary, id: &str) -> Option<&'a clean::RuleSummary> {
    plan.groups
        .iter()
        .flat_map(|g| &g.rules)
        .find(|r| r.id == id)
}

fn run(h: &Home, plan: &PlanSummary, ids: &[&str]) -> clean::Outcome {
    run_except(h, plan, ids, &[])
}

fn run_except(
    h: &Home,
    plan: &PlanSummary,
    ids: &[&str],
    skip: &[std::path::PathBuf],
) -> clean::Outcome {
    let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    clean::execute(
        &h.ctx,
        plan.plan,
        &ids,
        skip,
        &HashMap::new(),
        &AtomicBool::new(false),
        &mut |_, _| {},
    )
    .unwrap()
}

const CACHE_RULE: &str = r#"
[[rule]]
id = "c"
group = "g"
title = "Caches"
paths = ["~/Library/Caches"]
"#;

#[test]
fn scan_measures_and_removes_nothing_then_clean_removes_only_selected() {
    let h = Home::new();
    write(&h.p("Library/Caches/app.one/data.bin"), 200_000);
    write(&h.p("Library/Caches/app.two/data.bin"), 100_000);
    write(&h.p("logs/old.log"), 50_000);
    let set = rules(&format!(
        "{CACHE_RULE}\n[[rule]]\nid = \"l\"\ngroup = \"g\"\ntitle = \"Logs\"\npaths = [\"~/logs\"]\n"
    ));
    let plan = scan(&h, &set, &Running::default());
    assert_eq!(rule(&plan, "c").unwrap().items, 2);
    assert!(rule(&plan, "c").unwrap().bytes >= 300_000);
    assert!(
        h.p("Library/Caches/app.one/data.bin").exists(),
        "a scan must not remove anything"
    );

    let out = run(&h, &plan, &["c"]);
    assert_eq!((out.removed, out.failed, out.skipped), (2, 0, 0));
    assert!(!h.p("Library/Caches/app.one").exists());
    assert!(
        h.p("Library/Caches").is_dir(),
        "the root folder itself stays"
    );
    assert!(
        h.p("logs/old.log").exists(),
        "an unselected rule keeps its files"
    );
}

#[test]
fn listed_items_come_biggest_first_and_unticked_ones_stay() {
    let h = Home::new();
    write(&h.p("Library/Caches/app.small/data.bin"), 10_000);
    write(&h.p("Library/Caches/app.big/data.bin"), 300_000);
    let plan = clean::scan_with(
        &h.ctx,
        &rules(CACHE_RULE),
        &Running::default(),
        true,
        &AtomicBool::new(false),
        &mut |_| {},
    );
    let listed = rule(&plan, "c").unwrap().paths.clone().unwrap();
    assert!(listed[0].0.ends_with("app.big") && listed[1].0.ends_with("app.small"));
    let out = run_except(&h, &plan, &["c"], &[h.p("Library/Caches/app.small")]);
    assert_eq!(out.removed, 1);
    assert!(!h.p("Library/Caches/app.big").exists());
    assert!(h.p("Library/Caches/app.small/data.bin").exists());
}

#[test]
fn an_item_replaced_after_the_scan_is_left_alone() {
    let h = Home::new();
    write(&h.p("Library/Caches/app.one/data.bin"), 10_000);
    let plan = scan(&h, &rules(CACHE_RULE), &Running::default());
    std::fs::remove_dir_all(h.p("Library/Caches/app.one")).unwrap();
    write(&h.p("Library/Caches/app.one/new-and-important.txt"), 10);
    let out = run(&h, &plan, &["c"]);
    assert_eq!((out.removed, out.skipped), (0, 1));
    assert!(h.p("Library/Caches/app.one/new-and-important.txt").exists());
}

#[cfg(unix)]
#[test]
fn a_folder_swapped_for_a_link_after_the_scan_cannot_redirect_the_removal() {
    let h = Home::new();
    write(&h.p("cache/sub/item/f"), 10_000);
    write(&h.outside("sub/item/precious"), 10);
    let set =
        rules("[[rule]]\nid = \"c\"\ngroup = \"g\"\ntitle = \"C\"\npaths = [\"~/cache/sub\"]\n");
    let plan = scan(&h, &set, &Running::default());
    assert_eq!(rule(&plan, "c").unwrap().items, 1);
    // attacker swaps ~/cache/sub for a link to a folder holding a same-named item
    std::fs::remove_dir_all(h.p("cache/sub")).unwrap();
    std::os::unix::fs::symlink(h.outside("sub"), h.p("cache/sub")).unwrap();
    let out = run(&h, &plan, &["c"]);
    assert_eq!(out.removed, 0);
    assert!(h.outside("sub/item/precious").exists());
}

#[cfg(unix)]
#[test]
fn links_are_never_followed() {
    let h = Home::new();
    write(&h.outside("target/precious"), 10_000);
    std::fs::create_dir_all(h.p("Library/Caches")).unwrap();
    // a link as an item: only the link goes
    std::os::unix::fs::symlink(h.outside("target"), h.p("Library/Caches/link-item")).unwrap();
    // a link where a pattern component should be: no match at all
    std::os::unix::fs::symlink(h.outside("target"), h.p("linked-root")).unwrap();
    let set = rules(&format!(
        "{CACHE_RULE}\n[[rule]]\nid = \"r\"\ngroup = \"g\"\ntitle = \"R\"\npaths = [\"~/linked-root\"]\n"
    ));
    let plan = scan(&h, &set, &Running::default());
    assert!(rule(&plan, "r").is_none());
    run(&h, &plan, &["c", "r"]);
    assert!(h.outside("target/precious").exists());
}

#[test]
fn personal_folders_and_kept_folders_are_refused() {
    let h = Home::new();
    write(&h.p("Documents/thesis.docx"), 10_000);
    write(&h.p("Desktop/photo.jpg"), 10_000);
    write(&h.p("Library/Caches/x/f"), 10_000);
    let set = rules(
        r#"
[[rule]]
id = "docs"
group = "g"
title = "Bad rule"
paths = ["~/Documents", "~/Desktop"]

[[rule]]
id = "caches-itself"
group = "g"
title = "Bad rule"
paths = ["~/Library/Caches"]
target = "self"
"#,
    );
    let plan = scan(&h, &set, &Running::default());
    assert_eq!(plan.items, 0);
    run(&h, &plan, &["docs", "caches-itself"]);
    assert!(h.p("Documents/thesis.docx").exists() && h.p("Desktop/photo.jpg").exists());
    assert!(h.p("Library/Caches/x/f").exists());
}

#[test]
fn running_apps_keep_their_caches() {
    let h = Home::new();
    write(&h.p("Library/Caches/com.example.live/f"), 10_000);
    write(&h.p("Library/Caches/com.example.idle/f"), 10_000);
    write(&h.p("browser/cache/f"), 10_000);
    let set = rules(
        r#"
[[rule]]
id = "c"
group = "g"
title = "Caches"
paths = ["~/Library/Caches"]
item_is_bundle_id = true

[[rule]]
id = "b"
group = "g"
title = "Browser"
paths = ["~/browser"]
running = ["com.example.Browser"]
"#,
    );
    let running = Running::of(&["com.example.live", "com.example.browser"]);
    let plan = scan(&h, &set, &running);
    assert_eq!(rule(&plan, "c").unwrap().items, 1);
    assert_eq!(rule(&plan, "c").unwrap().kept, 1);
    assert_eq!(rule(&plan, "b").unwrap().status, "in_use");
    run(&h, &plan, &["c", "b"]);
    assert!(h.p("Library/Caches/com.example.live/f").exists());
    assert!(!h.p("Library/Caches/com.example.idle").exists());
    assert!(h.p("browser/cache/f").exists());
}

#[test]
fn young_items_wait_for_their_age() {
    let h = Home::new();
    write(&h.p("logs/new.log"), 1000);
    write(&h.p("logs/old.log"), 1000);
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(10 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(h.p("logs/old.log"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    let set = rules(
        "[[rule]]\nid = \"l\"\ngroup = \"g\"\ntitle = \"L\"\npaths = [\"~/logs\"]\nmin_age_days = 7\n",
    );
    let plan = scan(&h, &set, &Running::default());
    assert_eq!(rule(&plan, "l").unwrap().items, 1);
    run(&h, &plan, &["l"]);
    assert!(h.p("logs/new.log").exists() && !h.p("logs/old.log").exists());
}

#[test]
fn overlapping_rules_count_a_path_once() {
    let h = Home::new();
    write(&h.p("Library/Caches/Google/Chrome/Default/f"), 100_000);
    write(&h.p("Library/Caches/other/f"), 100_000);
    let set = rules(
        r#"
[[rule]]
id = "chrome"
group = "g"
title = "Chrome"
paths = ["~/Library/Caches/Google/Chrome"]

[[rule]]
id = "c"
group = "g"
title = "Caches"
paths = ["~/Library/Caches"]
"#,
    );
    let plan = scan(&h, &set, &Running::default());
    assert_eq!(rule(&plan, "chrome").unwrap().items, 1);
    // "Google" contains Chrome's claimed folder, so the general rule leaves it to Chrome's
    assert_eq!(rule(&plan, "c").unwrap().items, 1);
    let total: u64 = plan
        .groups
        .iter()
        .flat_map(|g| &g.rules)
        .map(|r| r.bytes)
        .sum();
    assert!(total < 300_000, "{total}");
}

#[test]
fn excludes_and_includes_pick_items_by_name() {
    let h = Home::new();
    for n in ["keep.db", "thumbcache_1.db", "thumbcache_2.db"] {
        write(&h.p(&format!("explorer/{n}")), 5000);
    }
    let set = rules(
        "[[rule]]\nid = \"t\"\ngroup = \"g\"\ntitle = \"T\"\npaths = [\"~/explorer\"]\ninclude = [\"thumbcache_*.db\"]\nexclude = [\"*_2.db\"]\n",
    );
    let plan = scan(&h, &set, &Running::default());
    assert_eq!(rule(&plan, "t").unwrap().items, 1);
    run(&h, &plan, &["t"]);
    assert!(h.p("explorer/keep.db").exists() && h.p("explorer/thumbcache_2.db").exists());
    assert!(!h.p("explorer/thumbcache_1.db").exists());
}

#[test]
fn every_attempt_is_in_the_history() {
    let h = Home::new();
    write(&h.p("Library/Caches/a/f"), 10_000);
    let plan = scan(&h, &rules(CACHE_RULE), &Running::default());
    let out = run(&h, &plan, &["c"]);
    assert_eq!(out.removed, 1);
    let history = oc_engine::ledger::sessions(&h.ctx, 10);
    assert_eq!(history["sessions"][0]["command"], "clean");
    assert_eq!(history["sessions"][0]["items"], 1);
    // path separators differ by platform: compare the logged path, not its text
    let log = std::fs::read_to_string(oc_engine::ledger::path(&h.ctx)).unwrap();
    let removed = log
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .any(|v| {
            v["result"] == "removed"
                && v["path"].as_str().is_some_and(|p| {
                    Path::new(p).ends_with(Path::new("Library").join("Caches").join("a"))
                })
        });
    assert!(removed, "{log}");
}

#[test]
fn an_expired_plan_cannot_be_executed() {
    let h = Home::new();
    let err = clean::execute(
        &h.ctx,
        999_999,
        &["c".into()],
        &[],
        &HashMap::new(),
        &AtomicBool::new(false),
        &mut |_, _| {},
    );
    assert!(err.is_err());
}

#[test]
fn rule_files_are_validated() {
    assert!(
        RuleSet::parse(&format!(
            "{GROUPS}[[rule]]\nid=\"x\"\ngroup=\"g\"\ntitle=\"X\"\npaths=[\"~/a/../b\"]\n"
        ))
        .is_err()
    );
    assert!(
        RuleSet::parse(&format!(
            "{GROUPS}[[rule]]\nid=\"x\"\ngroup=\"g\"\ntitle=\"X\"\npaths=[\"/etc\"]\n"
        ))
        .is_err()
    );
    assert!(
        RuleSet::parse(&format!(
            "{GROUPS}[[rule]]\nid=\"x\"\ngroup=\"nope\"\ntitle=\"X\"\npaths=[\"~/a\"]\n"
        ))
        .is_err()
    );
    assert!(
        RuleSet::parse(&format!(
            "{GROUPS}[[rule]]\nid=\"x\"\ngroup=\"g\"\ntitle=\"X\"\npaths=[\"~/a\"]\ntypo=1\n"
        ))
        .is_err()
    );
    // both shipped packs load, whatever platform runs the tests
    for pack in [
        include_str!("../rules/macos.toml"),
        include_str!("../rules/windows.toml"),
    ] {
        let set = RuleSet::parse(pack).unwrap();
        assert!(!set.rules.is_empty());
    }
}

#[test]
fn shipped_rules_never_aim_at_personal_folders() {
    for pack in [
        include_str!("../rules/macos.toml"),
        include_str!("../rules/windows.toml"),
    ] {
        for r in RuleSet::parse(pack).unwrap().rules {
            for p in &r.paths {
                // whole path components: "Mail Downloads" is not the Downloads folder
                let parts: Vec<String> = p
                    .split(['/', '\\'])
                    .map(|c| c.to_ascii_lowercase())
                    .collect();
                for bad in [
                    "documents",
                    "desktop",
                    "pictures",
                    "downloads",
                    "mobile documents",
                    "keychains",
                    ".ssh",
                ] {
                    assert!(!parts.iter().any(|c| c == bad), "{}: {p}", r.id);
                }
            }
        }
    }
}

#[test]
fn privacy_rules_are_never_preselected_and_wait_for_their_browser() {
    for pack in [
        include_str!("../rules/macos.toml"),
        include_str!("../rules/windows.toml"),
    ] {
        let set = RuleSet::parse(pack).unwrap();
        let privacy: Vec<_> = set.rules.iter().filter(|r| r.group == "privacy").collect();
        // Chrome, Edge, Firefox everywhere; history and cookies are separate choices
        assert!(privacy.len() >= 6);
        for r in &privacy {
            assert!(!r.default, "{} must never be preselected", r.id);
            assert!(!r.running.is_empty(), "{} must wait for its browser", r.id);
            assert_eq!(r.target, oc_engine::rules::Target::Itself, "{}", r.id);
            assert!(!r.note.is_empty(), "{} must say what is lost", r.id);
            assert!(
                r.id.ends_with(".history") || r.id.ends_with(".cookies"),
                "{}",
                r.id
            );
        }
        assert!(!privacy.iter().any(|r| r.id.contains("safari")));
    }
}

#[test]
fn browser_history_goes_with_its_sidecar_files_and_cookies_stay() {
    let h = Home::new();
    let profile = "Library/Application Support/Google/Chrome/Default";
    write(&h.p(&format!("{profile}/History")), 4000);
    write(&h.p(&format!("{profile}/History-journal")), 100);
    write(&h.p(&format!("{profile}/Cookies")), 2000);
    write(&h.p(&format!("{profile}/Bookmarks")), 500);
    // the removal checks the real running apps again: name a browser that is never running
    let pack = include_str!("../rules/macos.toml").replace("com.google.Chrome", "oc.test.browser");
    let set = RuleSet::parse(&pack).unwrap();

    let busy = scan(&h, &set, &Running::of(&["oc.test.browser"]));
    assert_eq!(
        rule(&busy, "privacy.chrome.history").unwrap().status,
        "in_use"
    );

    let plan = scan(&h, &set, &Running::default());
    assert_eq!(rule(&plan, "privacy.chrome.history").unwrap().items, 2);
    assert_eq!(rule(&plan, "privacy.chrome.cookies").unwrap().items, 1);
    run(&h, &plan, &["privacy.chrome.history"]);
    assert!(!h.p(&format!("{profile}/History")).exists());
    assert!(!h.p(&format!("{profile}/History-journal")).exists());
    assert!(
        h.p(&format!("{profile}/Cookies")).exists(),
        "cookies were not chosen"
    );
    assert!(h.p(&format!("{profile}/Bookmarks")).exists());
}

fn scan_events(h: &Home, set: &RuleSet) -> (Vec<PlanSummary>, PlanSummary) {
    let mut events = Vec::new();
    let plan = clean::scan(
        &h.ctx,
        set,
        &Running::default(),
        &AtomicBool::new(false),
        &mut |p| events.push(p.clone()),
    );
    (events, plan)
}

#[test]
fn a_rescan_shows_the_last_sizes_flagged_as_cached_then_the_exact_plan() {
    let h = Home::new();
    let set = rules(CACHE_RULE);
    write(&h.p("Library/Caches/app.one/data.bin"), 100_000);
    let (events, first) = scan_events(&h, &set);
    assert!(events.iter().all(|e| !e.cached), "nothing cached yet");
    assert!(!first.cached);

    // the cache grows in place: same folder, more bytes
    write(&h.p("Library/Caches/app.one/data.bin"), 900_000);
    let (events, plan) = scan_events(&h, &set);
    let shown = events.first().expect("the cached sizes are shown at once");
    assert!(shown.cached && shown.measured_at > 0);
    assert_eq!(
        shown.bytes, first.bytes,
        "the previous scan's size, flagged as such"
    );
    assert!(!plan.cached, "the plan is always measured");
    assert!(
        plan.bytes >= 900_000,
        "the exact size replaces the cached one"
    );
}

#[test]
fn execute_reports_measured_bytes_never_cached_ones() {
    let h = Home::new();
    let set = rules(CACHE_RULE);
    write(&h.p("Library/Caches/app.one/data.bin"), 100_000);
    scan_events(&h, &set);
    // a cache claiming far more than is there
    let file = h.ctx.data_dir.join("junk-sizes-v1.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    v["items"][0][3] = serde_json::json!(1u64 << 40);
    std::fs::write(&file, v.to_string()).unwrap();

    let (events, plan) = scan_events(&h, &set);
    assert!(events.iter().any(|e| e.cached && e.bytes == 1 << 40));
    assert!(plan.bytes < 1 << 30);
    let out = run(&h, &plan, &["c"]);
    assert_eq!(out.removed, 1);
    assert_eq!(
        out.bytes, plan.bytes,
        "freed bytes come from the exact measurement"
    );
}

#[allow(dead_code)]
fn exists(p: &Path) -> bool {
    p.exists()
}

/// Uses the real Trash: run with `cargo test -- --ignored trash`.
#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn trash_reports_what_it_cannot_move_and_leaves_no_copy() {
    use std::os::unix::fs::PermissionsExt;
    let t = TempDir::new();
    let locked = t.path().join("locked");
    let name = format!("oc-locked-{}.txt", std::process::id());
    write(&locked.join(&name), 1000);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let res = oc_engine::trash::move_to_trash(&locked.join(&name));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        res.is_err(),
        "a file that could not be moved was reported as trashed"
    );
    assert!(locked.join(&name).exists());
    let trash = Path::new(&std::env::var("HOME").unwrap()).join(".Trash");
    let leftovers: Vec<_> = std::fs::read_dir(&trash)
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("oc-locked-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "a copy was left in the Trash: {leftovers:?}"
    );

    // a file on the read-only system volume: macOS copies it to the Trash and says it
    // trashed it; we must report a failure and leave no copy behind
    let before = std::fs::read_dir(&trash).unwrap().count();
    assert!(oc_engine::trash::move_to_trash(Path::new("/usr/bin/true")).is_err());
    assert!(Path::new("/usr/bin/true").exists());
    assert_eq!(
        std::fs::read_dir(&trash).unwrap().count(),
        before,
        "a copy of /usr/bin/true was left in the Trash"
    );

    // and a normal file really goes
    let free = t.path().join(format!("oc-free-{}.txt", std::process::id()));
    write(&free, 10);
    oc_engine::trash::move_to_trash(&free).unwrap();
    assert!(!free.exists());
    let _ = std::fs::remove_file(trash.join(free.file_name().unwrap()));
}

#[test]
fn shipped_rules_are_consistent() {
    for pack in [
        include_str!("../rules/macos.toml"),
        include_str!("../rules/windows.toml"),
    ] {
        let set = RuleSet::parse(pack).unwrap();
        let mut seen = std::collections::HashMap::new();
        for r in &set.rules {
            assert!(!r.note.is_empty(), "{}: says what cleaning it costs", r.id);
            for p in &r.paths {
                // one place, one rule: two rules on one path count it twice
                if let Some(other) = seen.insert(p.to_ascii_lowercase(), &r.id) {
                    panic!("{p} is in both {other} and {}", r.id);
                }
                // an app's own data folder is cleaned only while that app is closed
                let app_data = p.contains("Application Support/")
                    || p.starts_with("%APPDATA%")
                    || p.starts_with("%LOCALAPPDATA%/Programs");
                if app_data && ["apps", "developer", "browsers"].contains(&r.group.as_str()) {
                    assert!(!r.running.is_empty(), "{}: {p} needs the app it waits for", r.id);
                }
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn removes_trees_with_read_only_folders() {
    use std::os::unix::fs::PermissionsExt;
    let h = Home::new();
    let inner = h.p("tool/pkg/inner");
    write(&inner.join("f"), 1000);
    // like dotslash and the Go module cache: folders without write permission
    for d in [inner.clone(), h.p("tool/pkg")] {
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    let set = rules("[[rule]]\nid = \"t\"\ngroup = \"g\"\ntitle = \"T\"\npaths = [\"~/tool\"]\n");
    let plan = scan(&h, &set, &Running::default());
    let out = run(&h, &plan, &["t"]);
    assert_eq!((out.removed, out.failed), (1, 0));
    assert!(!h.p("tool/pkg").exists());
}
