//! `oce`: the engine on the command line, for benchmarks, scripts and Windows work.
//!
//!   oce version
//!   oce size [--freed] [--all-volumes] [--json] PATH...   du-style "<KiB>\t<path>" lines
//!   oce space [PATH]                                       Space Lens listing (JSON); no PATH: overview
//!   oce scan                                               System Junk preview (removes nothing)
//!   oce hogs [--json]                                      stuck, heavy and left-behind processes
//!   oce quit [--force] PID                                 quit one of them (asks first; --force kills)
//!   oce call '<request json>'                              same requests as the app
//!   oce run '<request json>'                               a job: one event per line

use oc_engine::size::{self, Mode};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("version") => {
            println!("{}", oc_engine::VERSION);
            ExitCode::SUCCESS
        }
        Some("size") => size_cmd(&args[1..]),
        Some("space") => {
            let mut req = serde_json::json!({"op": "space"});
            if let Some(p) = args.get(1) {
                req["path"] = serde_json::json!(p);
            }
            let start = std::time::Instant::now();
            let cancel = std::sync::atomic::AtomicBool::new(false);
            oc_engine::api::run(&req.to_string(), &cancel, &mut |event| {
                if event["event"] != "progress" && event["event"] != "done" {
                    println!("{event}");
                }
            });
            eprintln!("oce space: {:.2}s", start.elapsed().as_secs_f64());
            ExitCode::SUCCESS
        }
        Some("status") => {
            let start = std::time::Instant::now();
            println!("{}", oc_engine::api::call(r#"{"op":"status"}"#));
            eprintln!(
                "oce status: {:.0} ms",
                start.elapsed().as_secs_f64() * 1000.0
            );
            ExitCode::SUCCESS
        }
        Some("hogs") => hogs_cmd(args.iter().any(|a| a == "--json")),
        Some("quit") => quit_cmd(&args[1..]),
        Some("scan") => {
            let start = std::time::Instant::now();
            let cancel = std::sync::atomic::AtomicBool::new(false);
            oc_engine::api::run(r#"{"op":"scan"}"#, &cancel, &mut |event| {
                if event["event"] == "plan" {
                    for g in event["groups"].as_array().into_iter().flatten() {
                        println!("{}", g["title"].as_str().unwrap_or(""));
                        for r in g["rules"].as_array().into_iter().flatten() {
                            println!(
                                "  {:<44} {:>9.1} MB {:>6} items  {}{}",
                                r["title"].as_str().unwrap_or(""),
                                r["bytes"].as_f64().unwrap_or(0.0) / 1e6,
                                r["items"],
                                r["status"].as_str().unwrap_or(""),
                                if r["default"] == true {
                                    ""
                                } else {
                                    "  (not preselected)"
                                }
                            );
                        }
                    }
                    println!(
                        "total {:.2} GB in {} items",
                        event["bytes"].as_f64().unwrap_or(0.0) / 1e9,
                        event["items"]
                    );
                }
            });
            eprintln!("oce scan: {:.2}s", start.elapsed().as_secs_f64());
            ExitCode::SUCCESS
        }
        Some(cmd @ ("apps" | "installers" | "projects")) => {
            let start = std::time::Instant::now();
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let req = format!(r#"{{"op":"{cmd}"}}"#);
            oc_engine::api::run(&req, &cancel, &mut |event| match event["event"].as_str() {
                Some("apps") => eprintln!(
                    "{} apps listed after {:.0} ms",
                    event["apps"].as_array().map_or(0, |a| a.len()),
                    start.elapsed().as_secs_f64() * 1000.0
                ),
                Some("sizes") if event["final"] == true => {
                    let total: u64 = event["sizes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_u64())
                        .sum();
                    println!("apps total {:.2} GB", total as f64 / 1e9);
                }
                Some("picks") => {
                    for p in event["items"].as_array().into_iter().flatten() {
                        println!(
                            "{} {:>9.1} MB  {:<40} {}  ({})",
                            if p["preselect"] == true { "[x]" } else { "[ ]" },
                            p["bytes"].as_f64().unwrap_or(0.0) / 1e6,
                            p["group"].as_str().unwrap_or(""),
                            p["name"].as_str().unwrap_or(""),
                            p["detail"].as_str().unwrap_or("")
                        );
                    }
                    println!(
                        "{} items, {:.2} GB",
                        event["items"].as_array().map_or(0, |a| a.len()),
                        event["bytes"].as_f64().unwrap_or(0.0) / 1e9
                    );
                }
                _ => {}
            });
            eprintln!("oce {cmd}: {:.2}s", start.elapsed().as_secs_f64());
            ExitCode::SUCCESS
        }
        Some("run") if args.len() == 2 => {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            oc_engine::api::run(&args[1], &cancel, &mut |event| println!("{event}"));
            ExitCode::SUCCESS
        }
        Some("call") if args.len() == 2 => {
            println!("{}", oc_engine::api::call(&args[1]));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "usage: oce version | oce size [--freed] [--all-volumes] [--json] PATH... | oce hogs [--json] | oce quit [--force] PID | oce call JSON | oce run JSON"
            );
            ExitCode::from(2)
        }
    }
}

fn size_cmd(args: &[String]) -> ExitCode {
    let mut opts = size::Options::default();
    let mut json = false;
    let mut paths = Vec::new();
    for a in args {
        match a.as_str() {
            "--freed" => opts.mode = Mode::Freed,
            "--all-volumes" => opts.one_volume = false,
            "--json" => json = true,
            _ => paths.push(PathBuf::from(a)),
        }
    }
    let links = size::Links::default();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut status = ExitCode::SUCCESS;
    for path in paths {
        match size::measure_with(&path, &opts, &links, &cancel) {
            Ok(r) => {
                if json {
                    println!("{}", serde_json::json!({"path": path, "report": r}));
                } else {
                    println!("{}\t{}", size::kib(r.bytes), path.display());
                }
                if !r.complete() {
                    if let Some(e) = &r.first_error {
                        eprintln!("oce: {e}");
                    }
                    status = ExitCode::from(1);
                }
            }
            Err(e) => {
                eprintln!("oce: {}: {e}", path.display());
                status = ExitCode::from(1);
            }
        }
    }
    status
}

fn hogs_cmd(json: bool) -> ExitCode {
    use oc_engine::hogs::{self, Reason, Verdict};
    let report = hogs::read();
    if json {
        println!("{}", serde_json::to_string(&report).unwrap_or_default());
        return ExitCode::SUCCESS;
    }
    let gb = |b: u64| b as f64 / 1e9;
    let m = &report.machine;
    println!(
        "memory {:.1} GB, swap {:.1}/{:.1} GB{}",
        gb(m.total_ram),
        gb(m.swap_used),
        gb(m.swap_total),
        if m.under_pressure {
            "  (short of memory)"
        } else {
            ""
        }
    );
    for f in &report.findings {
        let why: Vec<&str> = f
            .reasons
            .iter()
            .map(|r| match r {
                Reason::Zombie => "zombie",
                Reason::Stopped => "stopped",
                Reason::Spinning => "stuck in a loop",
                Reason::HeavyMemory => "heavy",
                Reason::LeftBehind => "left behind",
                Reason::IdleDevTool => "idle dev tool",
            })
            .collect();
        println!(
            "{:<6} {:>7} {:>6.1} GB {:>5.0}% {:>6}  {:<24} {}",
            match f.verdict {
                Verdict::Quit => "QUIT",
                Verdict::Review => "review",
            },
            f.pid,
            gb(f.memory_bytes),
            f.cpu,
            if f.age_secs >= 86_400 {
                format!("{}d", f.age_secs / 86_400)
            } else {
                format!("{}h", f.age_secs / 3600)
            },
            f.app.chars().take(24).collect::<String>(),
            why.join(", ")
        );
    }
    for g in &report.groups {
        println!(
            "group  {:>3} x {:<24} {:.1} GB",
            g.count,
            g.app,
            gb(g.memory_bytes)
        );
    }
    println!(
        "quitting all of them would free about {:.1} GB",
        gb(report.reclaimable)
    );
    ExitCode::SUCCESS
}

fn quit_cmd(args: &[String]) -> ExitCode {
    let force = args.iter().any(|a| a == "--force");
    let Some(pid) = args.iter().find_map(|a| a.parse::<i32>().ok()) else {
        eprintln!("usage: oce quit [--force] PID");
        return ExitCode::from(2);
    };
    let Some(started) = oc_engine::hogs::start_of(pid) else {
        eprintln!("oce: no process {pid}");
        return ExitCode::from(1);
    };
    match oc_engine::hogs::quit(pid, started, force) {
        Ok(oc_engine::hogs::Outcome::Exited) => {
            println!("{pid} has quit");
            ExitCode::SUCCESS
        }
        Ok(oc_engine::hogs::Outcome::StillRunning) => {
            eprintln!("oce: {pid} is still running; try --force");
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("oce: {e}");
            ExitCode::from(1)
        }
    }
}
