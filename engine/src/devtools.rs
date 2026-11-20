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
    /// "docker", "simulators" or "android"
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
    /// An Android SDK is installed
    pub android: bool,
    /// Why Android items are not offered (empty when they are)
    pub android_note: String,
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
    if let Some(sdk) = android_sdk(ctx) {
        out.android = true;
        if sdkmanager(&sdk).is_some() && java_home().is_some() {
            out.items.extend(android_items(ctx, &sdk));
        } else {
            out.android_note = "Unused system images and old NDKs are offered once Android Studio (or the SDK Command-line Tools and a Java runtime) is installed.".into();
        }
    }
    out
}

/// ANDROID_HOME, ANDROID_SDK_ROOT, else where Android Studio installs it.
fn android_sdk(ctx: &Context) -> Option<PathBuf> {
    let env = ["ANDROID_HOME", "ANDROID_SDK_ROOT"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from);
    let default = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Android/Sdk"))
    } else {
        Some(ctx.home.join("Library/Android/sdk"))
    };
    env.chain(default)
        .find(|p| p.join("platform-tools").is_dir() || p.join("system-images").is_dir())
}

fn sdkmanager(sdk: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "sdkmanager.bat"
    } else {
        "sdkmanager"
    };
    Some(sdk.join("cmdline-tools/latest/bin").join(name)).filter(|p| p.is_file())
}

/// JAVA_HOME, else the runtime Android Studio ships (sdkmanager needs one).
fn java_home() -> Option<PathBuf> {
    let bundled = if cfg!(windows) {
        PathBuf::from("C:/Program Files/Android/Android Studio/jbr")
    } else {
        PathBuf::from("/Applications/Android Studio.app/Contents/jbr/Contents/Home")
    };
    std::env::var_os("JAVA_HOME")
        .map(PathBuf::from)
        .into_iter()
        .chain([bundled])
        .find(|p| p.join("bin").is_dir())
}

/// Where emulators (AVDs) live: ANDROID_AVD_HOME, ANDROID_USER_HOME/avd, ~/.android/avd.
fn avd_home(ctx: &Context) -> PathBuf {
    if let Some(p) = std::env::var_os("ANDROID_AVD_HOME") {
        return p.into();
    }
    std::env::var_os("ANDROID_USER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| ctx.home.join(".android"))
        .join("avd")
}

/// System images no emulator uses, and NDKs older than the newest one installed.
fn android_items(ctx: &Context, sdk: &Path) -> Vec<Item> {
    let used = used_system_images(&avd_home(ctx));
    let size = |p: &Path| {
        let opts = crate::size::Options {
            mode: crate::size::Mode::Freed,
            ..Default::default()
        };
        crate::size::measure(p, &opts).map_or(0, |r| r.bytes)
    };
    let dirs = |p: &Path| -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        v.sort();
        v
    };
    let mut items = Vec::new();
    // system-images/<api>/<tag>/<abi>, only when the emulators could be read
    let images = if used.is_some() {
        dirs(&sdk.join("system-images"))
    } else {
        Vec::new()
    };
    let used = used.unwrap_or_default();
    for api in images {
        for tag in dirs(&api) {
            for abi in dirs(&tag) {
                let rel: Vec<String> = [&api, &tag, &abi]
                    .iter()
                    .map(|p| {
                        p.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect();
                if used.contains(&rel.join("/")) {
                    continue;
                }
                let Some(package) = android_package(&["system-images", &rel[0], &rel[1], &rel[2]])
                else {
                    continue;
                };
                items.push(Item {
                    id: format!("android.pkg:{package}"),
                    group: "android",
                    title: "Unused emulator system image",
                    note: "No emulator uses it. Android Studio's Device Manager downloads it again if you need it.",
                    detail: format!("{} · {} · {}", rel[0], rel[1].replace('_', " "), rel[2]),
                    bytes: size(&abi),
                    default: false,
                });
            }
        }
    }
    // ndk/<version>: every one but the newest
    let mut ndks: Vec<(Vec<u64>, PathBuf)> = dirs(&sdk.join("ndk"))
        .into_iter()
        .map(|p| {
            let v = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .split('.')
                .map(|n| n.parse().unwrap_or(0))
                .collect();
            (v, p)
        })
        .collect();
    ndks.sort();
    ndks.pop();
    for (_, path) in ndks {
        let version = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some(package) = android_package(&["ndk", &version]) else {
            continue;
        };
        items.push(Item {
            id: format!("android.pkg:{package}"),
            group: "android",
            title: "Older NDK",
            note: "A newer NDK is installed. A project that pins this version (ndkVersion) downloads it again when it builds.",
            detail: version,
            bytes: size(&path),
            default: false,
        });
    }
    items
}

/// "system-images;android-36.1;google_apis;arm64-v8a", when every part is a plain name
/// (it becomes an argument of sdkmanager).
fn android_package(parts: &[&str]) -> Option<String> {
    let plain = |p: &&str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    parts.iter().all(plain).then(|| parts.join(";"))
}

/// "android-37.0/google_apis_playstore/arm64-v8a" of every AVD's image.sysdir.1: the AVD
/// folders in `avd_home` and wherever a `<name>.ini` there points (`path=`). None when the
/// folder cannot be read: then no image is offered, rather than all of them.
pub fn used_system_images(avd_home: &Path) -> Option<HashSet<String>> {
    let mut folders: Vec<PathBuf> = Vec::new();
    for e in std::fs::read_dir(avd_home).ok()?.flatten() {
        let path = e.path();
        if path.extension().is_some_and(|x| x == "ini") {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            folders.extend(
                text.lines()
                    .filter_map(|l| l.strip_prefix("path="))
                    .map(|p| PathBuf::from(p.trim())),
            );
        } else {
            folders.push(path);
        }
    }
    let mut used = HashSet::new();
    for folder in folders {
        let Ok(text) = std::fs::read_to_string(folder.join("config.ini")) else {
            continue;
        };
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("image.sysdir.1=") {
                // relative to the SDK, or absolute: what follows "system-images/"
                let v = v.trim().replace('\\', "/");
                let v = v
                    .rsplit_once("system-images/")
                    .map_or(v.as_str(), |(_, rest)| rest);
                used.insert(v.trim_end_matches('/').to_string());
            }
        }
    }
    Some(used)
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
        let Some((program, args)) = command(ctx, id, docker.as_deref()) else {
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
fn command(ctx: &Context, id: &str, docker: Option<&Path>) -> Option<(PathBuf, Vec<String>)> {
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let docker_args: &[&str] = match id {
        "docker.images" => &["image", "prune", "-a", "-f"],
        // -a: every unused layer, which is what `system df` counts as reclaimable
        "docker.build_cache" => &["builder", "prune", "-a", "-f"],
        "docker.containers" => &["container", "prune", "-f"],
        // no -a: named volumes stay (prune takes only anonymous ones)
        "docker.volumes" => &["volume", "prune", "-f"],
        _ if id.starts_with("android.pkg:") => {
            let sdk = android_sdk(ctx)?;
            return Some((
                sdkmanager(&sdk)?,
                args(&["--uninstall", id.strip_prefix("android.pkg:")?]),
            ));
        }
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
    let mut command = Command::new(program);
    if let Some(java) = java_home() {
        command.env("JAVA_HOME", java);
    }
    // docker.exe and sdkmanager.bat are console programs: no console window over the app
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
    let mut child = command
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
    // compared as text: these are paths on the daemon's (Unix) side, which a Windows
    // `Path` would not even call absolute
    if !socket.starts_with('/') || socket.split('/').any(|c| c == ".." || c == ".") {
        return false;
    }
    let under = |dir: &str| {
        let dir = dir.trim_end_matches('/');
        socket.len() > dir.len() && socket.starts_with(dir) && socket.as_bytes()[dir.len()] == b'/'
    };
    under(&home.to_string_lossy().replace('\\', "/"))
        || ["/var/run", "/private/var/run", "/run"]
            .iter()
            .any(|d| under(d))
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
    fn android_images_in_use() {
        let home = std::env::temp_dir().join(format!("oc-avd-{}", std::process::id()));
        let avd = home.join("Pixel.avd");
        std::fs::create_dir_all(&avd).unwrap();
        std::fs::write(
            avd.join("config.ini"),
            "AvdId=Pixel\nimage.sysdir.1=system-images/android-37.0/google_apis/arm64-v8a/\n",
        )
        .unwrap();
        // an AVD kept elsewhere, its image given as an absolute path
        let away = home.join("elsewhere/Tab.avd");
        std::fs::create_dir_all(&away).unwrap();
        std::fs::write(
            away.join("config.ini"),
            "image.sysdir.1=/sdk/system-images/android-35/default/x86_64/\n",
        )
        .unwrap();
        std::fs::write(home.join("Tab.ini"), format!("path={}\n", away.display())).unwrap();
        let used = used_system_images(&home).unwrap();
        assert!(used.contains("android-37.0/google_apis/arm64-v8a"));
        assert!(used.contains("android-35/default/x86_64"));
        assert_eq!(used.len(), 2);
        assert!(used_system_images(&home.join("missing")).is_none());
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(
            android_package(&["ndk", "27.0.1"]).as_deref(),
            Some("ndk;27.0.1")
        );
        assert_eq!(android_package(&["ndk", "x y"]), None);
    }

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
        let ctx = Context::from_env();
        let docker = Path::new("/usr/local/bin/docker");
        let (p, a) = command(&ctx, "docker.volumes", Some(docker)).unwrap();
        assert_eq!(p, docker);
        assert_eq!(a, ["volume", "prune", "-f"]);
        assert!(command(&ctx, "docker.images", None).is_none());
        let (p, a) = command(&ctx, "sim.runtime:84A4", None).unwrap();
        assert_eq!(p, Path::new(XCRUN));
        assert_eq!(a, ["simctl", "runtime", "delete", "84A4"]);
        assert!(command(&ctx, "rm -rf", None).is_none());
    }
}
