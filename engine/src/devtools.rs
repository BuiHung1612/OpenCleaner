//! Developer Tools: the space Docker and the iOS Simulator hold, reclaimed only with the
//! tools' own commands (never by deleting their files), and only when the user asks.
//!
//! `list` runs read-only commands (`docker system df`, `xcrun simctl list`); `clean` runs
//! the cleanup command of each chosen item, if a fresh listing still offers it.

use crate::context::Context;
use crate::ledger;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: String,
    /// "docker" or "simulators"
    pub group: &'static str,
    pub title: &'static str,
    pub note: &'static str,
    /// What exactly, when the title is generic ("iOS 26.5" for a runtime)
    pub detail: String,
    pub bytes: u64,
    pub default: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct Listing {
    /// "running", "stopped" (the CLI is there, the daemon does not answer), "remote" (the
    /// current context is not a daemon on this Mac: nothing offered) or "missing"
    pub docker: &'static str,
    /// The Docker context the commands go to ("default", "orbstack", "DOCKER_HOST"…)
    pub docker_context: String,
    /// Why some Docker items are not offered (empty when all are)
    pub docker_note: String,
    /// Xcode's simulators are set up for this user
    pub simulators: bool,
    /// What ~/Library/Developer/CoreSimulator/Devices holds (the devices' data)
    pub devices_bytes: u64,
    pub items: Vec<Item>,
}

#[derive(Debug, Default, Serialize)]
pub struct Outcome {
    pub freed: u64,
    pub ran: Vec<String>,
    pub failures: Vec<String>,
}

const LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// A cleanup command may take long (pruning many images), but a hung daemon must not
/// freeze the app: killed after this.
const CLEAN_TIMEOUT: Duration = Duration::from_secs(600);
const XCRUN: &str = "/usr/bin/xcrun";

pub fn list(ctx: &Context) -> Listing {
    let mut out = Listing {
        docker: "missing",
        ..Default::default()
    };
    if let Some(docker) = docker_cli(ctx) {
        let (context, endpoint) = docker_endpoint(&docker);
        out.docker_context = context;
        if !endpoint
            .as_deref()
            .is_some_and(|e| local_endpoint(e, &ctx.home))
        {
            // a remote (or unknown) daemon: its space is not this Mac's, never cleaned here
            out.docker = "remote";
            out.docker_note = format!(
                "Docker points to {}, not a daemon on this computer: nothing is cleaned there.",
                endpoint.as_deref().unwrap_or("an unknown daemon")
            );
        } else {
            match output(&docker, &["system", "df", "--format", "{{json .}}"]) {
                Some(text) => {
                    out.docker = "running";
                    out.items.extend(docker_items(&text));
                    let api = output(&docker, &["version", "--format", "{{.Server.APIVersion}}"]);
                    if !api.as_deref().is_some_and(keeps_named_volumes) {
                        // older engines' `volume prune` also deletes named volumes
                        out.items.retain(|i| i.id != "docker.volumes");
                        out.docker_note =
                            "Unused volumes are not offered: it needs Docker Engine 23 or newer."
                                .into();
                    }
                }
                None => out.docker = "stopped",
            }
        }
    }
    // without this folder, xcrun could offer to install the developer tools instead
    if cfg!(target_os = "macos") && ctx.home.join("Library/Developer/CoreSimulator").is_dir() {
        let xcrun = Path::new(XCRUN);
        let devices = output(xcrun, &["simctl", "list", "devices", "-j"]);
        let runtimes = output(xcrun, &["simctl", "runtime", "list", "-j"]);
        if let Some(devices) = devices.and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
            let d = parse_devices(&devices);
            out.simulators = true;
            out.devices_bytes = d.total;
            out.items.push(Item {
                id: "sim.unavailable".into(),
                group: "simulators",
                title: "Simulators of removed Xcode versions",
                note: "Their runtime is no longer installed, so they cannot run any more.",
                detail: String::new(),
                bytes: d.unavailable,
                default: true,
            });
            if let Some(r) = runtimes.and_then(|t| serde_json::from_str::<Value>(&t).ok()) {
                out.items.extend(unused_runtimes(&r, &d.used_runtimes));
            }
        }
    }
    out
}

/// Runs the cleanup of each chosen item still offered, and measures what it freed.
pub fn clean(ctx: &Context, ids: &[String]) -> Outcome {
    let started = ledger::now();
    let before = list(ctx);
    let docker = docker_cli(ctx);
    let mut out = Outcome::default();
    let mut done = Vec::new();
    for id in ids {
        // only what a fresh listing offers: an id never becomes an arbitrary argument
        if !before.items.iter().any(|i| &i.id == id) {
            out.failures.push(format!("{id}: not available"));
            continue;
        }
        let Some((program, args)) = command(id, docker.as_deref()) else {
            continue;
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match run(&program, &args, CLEAN_TIMEOUT) {
            Ok((true, _, _)) => {
                out.ran.push(id.clone());
                done.push(id);
            }
            Ok((false, _, stderr)) => out.failures.push(format!("{id}: {}", stderr.trim())),
            Err(e) => out.failures.push(format!("{id}: {e}")),
        }
    }
    let after = list(ctx);
    let bytes = |l: &Listing, id: &str| l.items.iter().find(|i| i.id == id).map_or(0, |i| i.bytes);
    out.freed = done
        .iter()
        .map(|id| bytes(&before, id).saturating_sub(bytes(&after, id)))
        .sum();
    ledger::Writer::open(ctx).write(&ledger::Line::Session {
        id: started,
        command: "devtools".into(),
        started,
        finished: ledger::now(),
        items: out.ran.len() as u64,
        bytes: out.freed,
        failed: out.failures.len() as u64,
    });
    out
}

/// The tool's own command for an item.
fn command(id: &str, docker: Option<&Path>) -> Option<(PathBuf, Vec<String>)> {
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let docker_args: &[&str] = match id {
        "docker.images" => &["image", "prune", "-a", "-f"],
        // -a: every unused layer, which is what `system df` counts as reclaimable
        "docker.build_cache" => &["builder", "prune", "-a", "-f"],
        "docker.containers" => &["container", "prune", "-f"],
        // no -a: named volumes stay (prune takes only anonymous ones)
        "docker.volumes" => &["volume", "prune", "-f"],
        "sim.unavailable" => {
            return Some((XCRUN.into(), args(&["simctl", "delete", "unavailable"])));
        }
        _ => {
            let runtime = id.strip_prefix("sim.runtime:")?;
            return Some((
                XCRUN.into(),
                args(&["simctl", "runtime", "delete", runtime]),
            ));
        }
    };
    Some((docker?.to_path_buf(), args(docker_args)))
}

fn docker_cli(ctx: &Context) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "docker.exe"
    } else {
        "docker"
    };
    // an app started from the Finder has a short PATH: also look where installers put it
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(name)).collect())
        .unwrap_or_default();
    candidates.extend(["/usr/local/bin/docker", "/opt/homebrew/bin/docker"].map(PathBuf::from));
    candidates.push(ctx.home.join(".docker/bin/docker"));
    candidates.push(ctx.home.join(".orbstack/bin/docker"));
    // is_file follows links: Docker's link into an unmounted disk image does not count
    candidates.into_iter().find(|p| p.is_file())
}

/// The program's folder first, so its helpers (credential stores, plugins) are found.
fn search_path(program: &Path) -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = program
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Standard output of a read-only command that succeeded within `LIST_TIMEOUT` (a hung
/// Docker daemon must not hang the page).
fn output(program: &Path, args: &[&str]) -> Option<String> {
    match run(program, args, LIST_TIMEOUT) {
        Ok((true, text, _)) => Some(text),
        _ => None,
    }
}

/// Runs a command, killed after `timeout`: (succeeded, stdout, stderr).
fn run(
    program: &Path,
    args: &[&str],
    timeout: Duration,
) -> std::io::Result<(bool, String, String)> {
    let mut child = Command::new(program)
        .args(args)
        .env("PATH", search_path(program))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_string(&mut s);
            }
            s
        })
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as _));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("did not finish within {} s; stopped", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    Ok((status.success(), out, err))
}

/// The current Docker context's name and daemon address: DOCKER_HOST when set, else
/// `docker context inspect` (the current context).
fn docker_endpoint(docker: &Path) -> (String, Option<String>) {
    if let Some(host) = std::env::var("DOCKER_HOST").ok().filter(|h| !h.is_empty()) {
        return ("DOCKER_HOST".into(), Some(host));
    }
    let text = output(
        docker,
        &[
            "context",
            "inspect",
            "--format",
            "{{.Name}} {{.Endpoints.docker.Host}}",
        ],
    )
    .unwrap_or_default();
    let mut parts = text.trim().splitn(2, ' ');
    let name = parts.next().unwrap_or("").to_string();
    let host = parts.next().map(str::trim).filter(|h| !h.is_empty());
    (name, host.map(String::from))
}

/// A daemon on this computer: a Unix socket in the user's home (Docker Desktop, OrbStack,
/// Colima, Rancher) or the system's run folder, or Docker Desktop's named pipe on Windows.
/// tcp:// and ssh:// (and anything else) may be another machine.
pub fn local_endpoint(endpoint: &str, home: &Path) -> bool {
    if let Some(pipe) = endpoint.strip_prefix("npipe://") {
        return pipe.replace('\\', "/").starts_with("//./pipe/");
    }
    let Some(socket) = endpoint.strip_prefix("unix://") else {
        return false;
    };
    let socket = Path::new(socket);
    // a plain path: no ".." to climb out of the allowed folders
    if !socket.is_absolute()
        || socket
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return false;
    }
    socket.starts_with(home)
        || ["/var/run", "/private/var/run", "/run"]
            .iter()
            .any(|d| socket.starts_with(d))
}

/// `docker version --format '{{.Server.APIVersion}}'` ("1.47"): API 1.42 (Engine 23) and
/// newer prune only anonymous volumes; older ones also delete named volumes.
pub fn keeps_named_volumes(api: &str) -> bool {
    let mut parts = api.trim().split('.').map(|p| p.parse::<u32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor))) => (major, minor) >= (1, 42),
        _ => false,
    }
}

/// `docker system df --format '{{json .}}'`: one object per line, sizes in Docker's
/// words ("1.2GB (50%)").
pub fn docker_items(text: &str) -> Vec<Item> {
    let mut items = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let bytes = parse_size(v["Reclaimable"].as_str().unwrap_or(""));
        let (id, title, note, default) = match v["Type"].as_str().unwrap_or("") {
            "Images" => (
                "docker.images",
                "Unused images",
                "Images no container uses. Docker downloads them again when needed.",
                true,
            ),
            "Build Cache" => (
                "docker.build_cache",
                "Build cache",
                "Cached build steps. The next build of each image takes longer.",
                true,
            ),
            "Containers" => (
                "docker.containers",
                "Stopped containers",
                "Containers that are not running, with any changes made inside them.",
                true,
            ),
            "Local Volumes" => (
                "docker.volumes",
                "Unused volumes",
                "Anonymous volumes no container uses. They can hold data such as databases, which is lost for good.",
                false,
            ),
            _ => continue,
        };
        items.push(Item {
            id: id.into(),
            group: "docker",
            title,
            note,
            detail: String::new(),
            bytes,
            default,
        });
    }
    items
}

/// Docker's human sizes: "0B", "12.5kB", "1.2GB (50%)", "3MiB".
pub fn parse_size(s: &str) -> u64 {
    let s = s.split_whitespace().next().unwrap_or("");
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let Ok(n) = number.parse::<f64>() else {
        return 0;
    };
    let power = match unit.to_ascii_lowercase().trim_end_matches('b') {
        "" => 0,
        "k" | "ki" => 1,
        "m" | "mi" => 2,
        "g" | "gi" => 3,
        "t" | "ti" => 4,
        "p" | "pi" => 5,
        _ => return 0,
    };
    let base: f64 = if unit.contains('i') { 1024.0 } else { 1000.0 };
    (n * base.powi(power)).round() as u64
}

#[derive(Debug, Default)]
pub struct Devices {
    pub total: u64,
    pub unavailable: u64,
    /// Runtimes at least one simulator is made for
    pub used_runtimes: HashSet<String>,
}

/// `xcrun simctl list devices -j`: {"devices": {"<runtime id>": [{isAvailable, dataPathSize}]}}
pub fn parse_devices(v: &Value) -> Devices {
    let mut d = Devices::default();
    for (runtime, list) in v["devices"].as_object().into_iter().flatten() {
        for device in list.as_array().into_iter().flatten() {
            let size = device["dataPathSize"].as_u64().unwrap_or(0);
            d.total += size;
            if device["isAvailable"].as_bool() == Some(false) {
                d.unavailable += size;
            }
            d.used_runtimes.insert(runtime.clone());
        }
    }
    d
}

/// `xcrun simctl runtime list -j`: {"<uuid>": {identifier, runtimeIdentifier, sizeBytes,
/// deletable, version}}. The deletable ones no simulator uses.
pub fn unused_runtimes(v: &Value, used: &HashSet<String>) -> Vec<Item> {
    let mut items: Vec<Item> = v
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(_, r)| {
            let identifier = r["identifier"].as_str()?;
            let runtime = r["runtimeIdentifier"].as_str()?;
            if used.contains(runtime) || r["deletable"].as_bool() != Some(true) {
                return None;
            }
            Some(Item {
                id: format!("sim.runtime:{identifier}"),
                group: "simulators",
                title: "Unused simulator runtime",
                note: "No simulator uses it. Xcode downloads it again if you need it.",
                detail: runtime_name(runtime, r["build"].as_str()),
                bytes: r["sizeBytes"].as_u64().unwrap_or(0),
                default: false,
            })
        })
        .collect();
    items.sort_by(|a, b| a.detail.cmp(&b.detail));
    items
}

/// "com.apple.CoreSimulator.SimRuntime.iOS-26-5" → "iOS 26.5 (23F77)"
fn runtime_name(runtime: &str, build: Option<&str>) -> String {
    let last = runtime.rsplit('.').next().unwrap_or(runtime);
    let name = match last.split_once('-') {
        Some((platform, version)) => format!("{platform} {}", version.replace('-', ".")),
        None => last.to_string(),
    };
    match build {
        Some(b) => format!("{name} ({b})"),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn docker_sizes() {
        assert_eq!(parse_size("0B"), 0);
        assert_eq!(parse_size("12.5kB"), 12_500);
        assert_eq!(parse_size("1.2GB (50%)"), 1_200_000_000);
        assert_eq!(parse_size("3MiB"), 3 * 1024 * 1024);
        assert_eq!(parse_size("2TB"), 2_000_000_000_000);
        assert_eq!(parse_size(""), 0);
        assert_eq!(parse_size("n/a"), 0);
    }

    #[test]
    fn docker_system_df() {
        let text = r#"{"Active":"2","Reclaimable":"1.2GB (50%)","Size":"2.4GB","TotalCount":"5","Type":"Images"}
{"Active":"1","Reclaimable":"0B (0%)","Size":"12kB","TotalCount":"3","Type":"Containers"}
{"Active":"1","Reclaimable":"100MB","Size":"300MB","TotalCount":"2","Type":"Local Volumes"}
{"Active":"0","Reclaimable":"512.3MB","Size":"512.3MB","TotalCount":"10","Type":"Build Cache"}
WARNING: something unrelated"#;
        let items = docker_items(text);
        let get = |id: &str| items.iter().find(|i| i.id == id).unwrap();
        assert_eq!(items.len(), 4);
        assert_eq!(get("docker.images").bytes, 1_200_000_000);
        assert_eq!(get("docker.containers").bytes, 0);
        assert_eq!(get("docker.build_cache").bytes, 512_300_000);
        // volumes can hold data: never ticked by default
        assert!(!get("docker.volumes").default);
        assert!(get("docker.images").default);
    }

    #[test]
    fn simulators() {
        let devices = json!({"devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0": [
                {"udid": "A", "isAvailable": true, "dataPathSize": 1000},
                {"udid": "B", "isAvailable": true, "dataPathSize": 500}
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-17-0": [
                {"udid": "C", "isAvailable": false, "dataPathSize": 300,
                 "availabilityError": "runtime profile not found"}
            ],
            "com.apple.CoreSimulator.SimRuntime.watchOS-11-0": []
        }});
        let d = parse_devices(&devices);
        assert_eq!(d.total, 1800);
        assert_eq!(d.unavailable, 300);
        assert!(
            d.used_runtimes
                .contains("com.apple.CoreSimulator.SimRuntime.iOS-27-0")
        );
        assert!(
            !d.used_runtimes
                .contains("com.apple.CoreSimulator.SimRuntime.watchOS-11-0")
        );

        let runtimes = json!({
            "84A4": {"identifier": "84A4", "runtimeIdentifier": "com.apple.CoreSimulator.SimRuntime.iOS-26-5",
                     "build": "23F77", "deletable": true, "sizeBytes": 8494282293u64, "version": "26.5"},
            "E743": {"identifier": "E743", "runtimeIdentifier": "com.apple.CoreSimulator.SimRuntime.iOS-27-0",
                     "build": "24A434", "deletable": true, "sizeBytes": 8067000161u64},
            "F000": {"identifier": "F000", "runtimeIdentifier": "com.apple.CoreSimulator.SimRuntime.tvOS-18-0",
                     "deletable": false, "sizeBytes": 1}
        });
        let items = unused_runtimes(&runtimes, &d.used_runtimes);
        assert_eq!(items.len(), 1, "in use and not deletable runtimes stay");
        assert_eq!(items[0].id, "sim.runtime:84A4");
        assert_eq!(items[0].detail, "iOS 26.5 (23F77)");
        assert_eq!(items[0].bytes, 8_494_282_293);
        assert!(!items[0].default);
    }

    #[test]
    fn volume_prune_needs_api_1_42() {
        assert!(keeps_named_volumes("1.47\n"));
        assert!(keeps_named_volumes("1.42"));
        assert!(keeps_named_volumes("2.0"));
        assert!(!keeps_named_volumes("1.41"));
        assert!(!keeps_named_volumes("1.9"));
        assert!(!keeps_named_volumes(""));
        assert!(!keeps_named_volumes("garbage"));
    }

    #[test]
    fn only_local_daemons() {
        let home = Path::new("/Users/me");
        assert!(local_endpoint("unix:///var/run/docker.sock", home));
        assert!(local_endpoint(
            "unix:///Users/me/.docker/run/docker.sock",
            home
        ));
        assert!(local_endpoint(
            "unix:///Users/me/.orbstack/run/docker.sock",
            home
        ));
        assert!(local_endpoint(
            "unix:///Users/me/.colima/default/docker.sock",
            home
        ));
        assert!(local_endpoint("npipe:////./pipe/docker_engine", home));
        assert!(!local_endpoint("tcp://10.0.0.5:2376", home));
        assert!(!local_endpoint("ssh://me@build-box", home));
        assert!(!local_endpoint("unix:///tmp/forwarded.sock", home));
        assert!(!local_endpoint(
            "unix:///Users/me/../other/docker.sock",
            home
        ));
        assert!(!local_endpoint("unix://relative.sock", home));
        assert!(!local_endpoint("", home));
    }

    #[test]
    fn a_hung_command_is_stopped() {
        if cfg!(windows) {
            return;
        }
        let started = Instant::now();
        let r = run(Path::new("/bin/sleep"), &["5"], Duration::from_millis(200));
        assert!(r.is_err_and(|e| e.kind() == std::io::ErrorKind::TimedOut));
        assert!(started.elapsed() < Duration::from_secs(3));
        let (ok, out, _) = run(Path::new("/bin/echo"), &["hi"], LIST_TIMEOUT).unwrap();
        assert!(ok && out.trim() == "hi");
    }

    #[test]
    fn commands_are_fixed() {
        let docker = Path::new("/usr/local/bin/docker");
        let (p, a) = command("docker.volumes", Some(docker)).unwrap();
        assert_eq!(p, docker);
        assert_eq!(a, ["volume", "prune", "-f"]);
        assert!(command("docker.images", None).is_none());
        let (p, a) = command("sim.runtime:84A4", None).unwrap();
        assert_eq!(p, Path::new(XCRUN));
        assert_eq!(a, ["simctl", "runtime", "delete", "84A4"]);
        assert!(command("rm -rf", None).is_none());
    }
}
