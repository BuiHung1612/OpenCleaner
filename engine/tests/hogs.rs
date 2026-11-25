//! Process Doctor: what `analyze` offers (made-up processes, so no machine state matters),
//! that protected processes are never offered, and that `quit` only ends the listed process.

use oc_engine::hogs::{Machine, Proc, Reason, State, Verdict, analyze};
use std::collections::HashMap;

const GIB: u64 = 1 << 30;
const NOW: u64 = 1_000_000;
const ME: i32 = 100;

fn proc(pid: i32, name: &str) -> Proc {
    Proc {
        pid,
        ppid: 1,
        name: name.into(),
        command: name.into(),
        app: None,
        mine: true,
        state: State::Sleeping,
        started: NOW - 60,
        rss: 50 << 20,
        cpu_ns: 0,
    }
}

fn machine() -> Machine {
    Machine {
        total_ram: 16 * GIB,
        ..Default::default()
    }
}

fn run(procs: &[Proc], cpu: &[(i32, f64)]) -> oc_engine::hogs::Report {
    let cpu: HashMap<i32, f64> = cpu.iter().copied().collect();
    analyze(procs, &cpu, NOW, &machine(), ME)
}

fn reasons(procs: &[Proc], cpu: &[(i32, f64)], pid: i32) -> Vec<Reason> {
    run(procs, cpu)
        .findings
        .into_iter()
        .find(|f| f.pid == pid)
        .map(|f| f.reasons)
        .unwrap_or_default()
}

#[test]
fn ordinary_processes_are_not_offered() {
    let procs = [proc(ME, "oce"), proc(200, "Notes"), proc(201, "Mail")];
    assert!(run(&procs, &[(200, 3.0)]).findings.is_empty());
}

#[test]
fn a_long_busy_loop_is_worth_quitting() {
    let mut p = proc(200, "worker");
    p.started = NOW - 3600;
    p.cpu_ns = 3500 * 1_000_000_000; // ~97 % of one core for the hour
    let report = run(&[p], &[(200, 99.0)]);
    let f = &report.findings[0];
    assert_eq!(f.verdict, Verdict::Quit);
    assert_eq!(f.reasons, [Reason::Spinning]);
}

#[test]
fn a_busy_spell_alone_is_not_a_loop() {
    // busy now, but only for the last minute of an hour-old process (a build, a render)
    let mut p = proc(200, "worker");
    p.started = NOW - 3600;
    p.cpu_ns = 60 * 1_000_000_000;
    assert!(run(&[p], &[(200, 99.0)]).findings.is_empty());
    // busy all its life but young: give it time
    let mut p = proc(201, "worker");
    p.started = NOW - 120;
    p.cpu_ns = 118 * 1_000_000_000;
    assert!(run(&[p], &[(201, 99.0)]).findings.is_empty());
}

#[test]
fn heavy_memory_is_left_to_the_user() {
    let mut p = proc(200, "Chrome Helper");
    p.rss = 3 * GIB;
    let report = run(&[p], &[]);
    assert_eq!(report.findings[0].verdict, Verdict::Review);
    assert_eq!(report.findings[0].reasons, [Reason::HeavyMemory]);
    assert_eq!(report.reclaimable, 3 * GIB);
}

#[test]
fn the_heavy_mark_drops_under_memory_pressure() {
    let mut p = proc(200, "Slack");
    p.rss = GIB / 2 + (100 << 20);
    let cpu = HashMap::new();
    let calm = analyze(&[p.clone()], &cpu, NOW, &machine(), ME);
    assert!(calm.findings.is_empty());
    let tight = Machine {
        under_pressure: true,
        ..machine()
    };
    assert_eq!(analyze(&[p], &cpu, NOW, &tight, ME).findings.len(), 1);
}

#[test]
fn a_dev_tool_left_behind_is_quit_but_a_daemon_by_design_is_only_reviewed() {
    let mut next = proc(200, "node");
    next.command = "node next dev --turbopack".into();
    next.started = NOW - 2 * 3600;
    let mut gradle = proc(201, "java");
    gradle.command = "java -Xmx4g org.gradle.launcher.daemon.bootstrap.GradleDaemon 8.9".into();
    gradle.started = NOW - 2 * 3600;
    let procs = [next, gradle.clone()];

    assert_eq!(reasons(&procs, &[], 200), [Reason::LeftBehind]);
    assert_eq!(run(&procs, &[]).findings.len(), 1);

    // the same daemon after a day idle is worth a look
    gradle.started = NOW - 30 * 3600;
    gradle.rss = 800 << 20;
    assert_eq!(reasons(&[gradle.clone()], &[], 201), [Reason::IdleDevTool]);

    // a tiny one is not worth the user's attention
    gradle.rss = 5 << 20;
    assert!(reasons(&[gradle], &[], 201).is_empty());
}

#[test]
fn a_dev_tool_with_a_living_parent_is_not_left_behind() {
    let mut parent = proc(300, "zsh");
    parent.ppid = 1;
    let mut next = proc(200, "node");
    next.command = "node next dev".into();
    next.ppid = 300;
    next.started = NOW - 2 * 3600;
    assert!(reasons(&[parent, next], &[], 200).is_empty());
}

#[test]
fn zombies_and_stopped_processes_are_reported() {
    let mut z = proc(200, "defunct");
    z.state = State::Zombie;
    let mut s = proc(201, "paused");
    s.state = State::Stopped;
    let report = run(&[z, s], &[]);
    let z = report.findings.iter().find(|f| f.pid == 200).unwrap();
    assert_eq!(z.reasons, [Reason::Zombie]);
    assert!(!z.can_quit);
    assert_eq!(
        report
            .findings
            .iter()
            .find(|f| f.pid == 201)
            .unwrap()
            .reasons,
        [Reason::Stopped]
    );
}

#[test]
fn never_offers_other_users_protected_or_parent_processes() {
    let heavy = |pid, name: &str| {
        let mut p = proc(pid, name);
        p.rss = 5 * GIB;
        p
    };
    let mut theirs = heavy(200, "postgres");
    theirs.mine = false;
    let mut terminal = heavy(300, "Terminal");
    terminal.ppid = 1;
    let mut shell = heavy(400, "zsh");
    shell.ppid = 300;
    let mut me = heavy(ME, "oce");
    me.ppid = 400;
    let procs = [
        theirs,
        terminal,
        shell,
        me,
        heavy(500, "Finder"),
        heavy(501, "WindowServer"),
        heavy(502, "explorer.exe"),
        heavy(1, "init"),
        heavy(600, "Photos"),
    ];
    let pids: Vec<i32> = run(&procs, &[]).findings.iter().map(|f| f.pid).collect();
    assert_eq!(pids, [600]);
}

#[test]
fn groups_sum_an_apps_processes() {
    let mut procs = Vec::new();
    for i in 0..5 {
        let mut p = proc(200 + i, "Google Chrome Helper");
        p.app = Some("Google Chrome".into());
        p.rss = 300 << 20;
        procs.push(p);
    }
    procs.push(proc(300, "Notes"));
    let report = run(&procs, &[]);
    assert_eq!(report.groups.len(), 1);
    assert_eq!(report.groups[0].app, "Google Chrome");
    assert_eq!(report.groups[0].count, 5);
    assert_eq!(report.groups[0].memory_bytes, 5 * (300 << 20));
}

#[test]
fn quit_users_first_then_biggest() {
    let mut big = proc(200, "big");
    big.rss = 4 * GIB;
    let mut spin = proc(201, "spin");
    spin.started = NOW - 3600;
    spin.cpu_ns = 3500 * 1_000_000_000;
    let report = run(&[big, spin], &[(201, 100.0)]);
    let order: Vec<i32> = report.findings.iter().map(|f| f.pid).collect();
    assert_eq!(order, [201, 200]);
}

#[cfg(unix)]
mod real {
    use oc_engine::hogs::{self, Outcome};
    use std::process::{Command, Stdio};

    #[test]
    fn quit_ends_only_the_listed_process() {
        let mut child = Command::new("sleep")
            .arg("60")
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let started = hogs::start_of(pid).expect("child is running");

        // a different start time means a reused pid: refused, and the process lives on
        assert!(hogs::quit(pid, started + 5, false).is_err());
        assert!(child.try_wait().unwrap().is_none());

        assert_eq!(hogs::quit(pid, started, false).unwrap(), Outcome::Exited);
        assert!(child.wait().is_ok());
    }

    #[test]
    fn quit_refuses_this_process_its_parents_and_init() {
        let me = std::process::id() as i32;
        let started = hogs::start_of(me).unwrap();
        assert!(hogs::quit(me, started, true).is_err());
        assert!(hogs::quit(1, 0, true).is_err());
    }
}
