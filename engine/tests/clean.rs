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
    let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    clean::execute(
        &h.ctx,
        plan.plan,
        &ids,
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
    let log = std::fs::read_to_string(oc_engine::ledger::path(&h.ctx)).unwrap();
    assert!(log.contains("\"result\":\"removed\"") && log.contains("Library/Caches/a"));
}

#[test]
fn an_expired_plan_cannot_be_executed() {
    let h = Home::new();
    let err = clean::execute(
        &h.ctx,
        999_999,
        &["c".into()],
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
                let lower = p.to_ascii_lowercase();
                for bad in [
                    "documents",
                    "desktop",
                    "pictures",
                    "downloads",
                    "mobile documents",
                    "keychains",
                    ".ssh",
                ] {
                    assert!(!lower.contains(bad), "{}: {p}", r.id);
                }
            }
        }
    }
}

#[allow(dead_code)]
fn exists(p: &Path) -> bool {
    p.exists()
}
