//! `oce`: the engine on the command line, for benchmarks, scripts and Windows work.
//!
//!   oce version
//!   oce size [--freed] [--all-volumes] [--json] PATH...   du-style "<KiB>\t<path>" lines
//!   oce space [PATH]                                       Space Lens listing (JSON); no PATH: overview
//!   oce scan                                               System Junk preview (removes nothing)
//!   oce call '<request json>'                              same requests as the app

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
        Some("call") if args.len() == 2 => {
            println!("{}", oc_engine::api::call(&args[1]));
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!(
                "usage: oce version | oce size [--freed] [--all-volumes] [--json] PATH... | oce call JSON"
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
