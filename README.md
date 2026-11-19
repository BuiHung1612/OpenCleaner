# OpenCleaner

Free, open-source cleaner for macOS (SwiftUI, Liquid Glass) and Windows (native Win32), on one Rust engine.
English is the default language; Vietnamese is included.

The engine is OpenCleaner's own, written in Rust (`engine/`) and linked into the app as a static library: no helper processes, one code base for macOS and Windows. It replaced [`mo`](https://github.com/tw93/mole), which earlier versions bundled. OpenCleaner is an independent project, not affiliated with or endorsed by the Mole project or MacPaw.

## Install

**macOS** (one line in Terminal; installs or updates to the latest release):

```sh
curl -fsSL https://raw.githubusercontent.com/BuiHung1612/OpenCleaner/main/install.sh | sh
```

[`install.sh`](install.sh) downloads `OpenCleaner.dmg`, checks the app's signature and copies it to `/Applications` (an older copy goes to the Trash). Or download `OpenCleaner.dmg` from [Releases](https://github.com/BuiHung1612/OpenCleaner/releases) and drag the app onto Applications: the app is not notarized, so the first launch of a browser download needs **System Settings → Privacy & Security → Open Anyway** once. After that the app updates itself.

**Windows:** download `OpenCleaner.exe` from [Releases](https://github.com/BuiHung1612/OpenCleaner/releases) and run it (SmartScreen: More info → Run anyway; the exe is not code-signed).

## Build (macOS 26+)

```sh
cd macos
./build.sh            # -> macos/build/OpenCleaner.app   (--debug, --universal)
                      #    builds the Rust engine library first (engine/build-lib.sh, needs cargo)
../engine/build-lib.sh && swift run   # quick dev run (rebuild the library after engine changes)
cd ../engine && cargo test             # engine tests (also run in CI on macOS and Windows)
```

## Build (Windows 10/11)

```sh
cd windows
cargo build --release   # -> windows\target\release\OpenCleaner.exe (icon and manifest embedded)
```

The Windows app is one executable: native Win32 painted with GDI+ (the same gradient pages, glass panels and big round button as on macOS), calling the engine directly. Pages: Dashboard, System Junk, Installers, Project Junk, Uninstaller (runs each app's own uninstaller), Space Lens (to the Recycle Bin) and History. English only for now.

## CI and releases

`.github/workflows/opencleaner.yml` tests the engine on macOS and Windows, builds the universal (Apple Silicon + Intel) macOS app and the Windows app on every push, and uploads `OpenCleaner.dmg` (macOS: drag the app onto Applications) and `OpenCleaner.exe` (Windows: runs as is) as workflow artifacts.

- **Release:** bump `VERSION`, commit, then push a matching tag: `git tag v0.1.0 && git push origin v0.1.0`. CI checks the tag matches `VERSION`, then creates a GitHub Release with `OpenCleaner.dmg` and `OpenCleaner.exe`.
- **Signing (optional, recommended):** run `macos/make-signing-cert.sh` once to create the fixed self-signed identity "OpenCleaner Code Signing", then `macos/make-signing-cert.sh --secrets OWNER/REPO` to upload `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD` and `MACOS_SIGN_ID`. A fixed signature keeps Full Disk Access granted across updates; ad-hoc builds lose it every release. It does not remove the first-launch Gatekeeper prompt (right-click → Open); that needs a Developer ID and notarization, not set up yet. Back up `~/.opencleaner-signing`: losing it means users grant access again once.

## Layout

| Path | What |
|---|---|
| `engine/src/fs/` | Bulk directory reading (`getattrlistbulk` on macOS, `FileIdBothDirectoryInfo` on Windows, readdir+lstat elsewhere) and the parallel tagged walker |
| `engine/src/size.rs` | du-compatible allocated sizes and "freed" sizes (APFS private size, hard links counted only when all links are inside) |
| `engine/src/space.rs`, `status.rs` | Space Lens listings and the Dashboard reading |
| `engine/rules/<platform>.toml` | **Cleaning rules as data**: where junk lives, which app owns it, how old it must be, what is preselected |
| `engine/src/clean.rs`, `rules.rs` | System Junk: scan → plan (nothing removed) → execute |
| `engine/src/apps.rs`, `finders.rs`, `slim.rs`, `maintenance.rs` | Uninstaller, Installers, Project Junk, Slim Apps, Optimization and Touch ID |
| `engine/src/guard.rs`, `picks.rs`, `trash.rs`, `ledger.rs` | The safety rules every removal passes, removal itself, Trash/Recycle Bin, history |
| `engine/src/api.rs`, `ffi.rs`, `include/oc_engine.h` | JSON requests and events over a small C ABI; `oce` (`src/bin/oce.rs`) is the same engine on the command line |
| `macos/Sources/OpenCleaner/Engine.swift` | Swift side of the C ABI: `Engine.call` (quick answers) and `Engine.run` (jobs as an `AsyncStream`) |
| `App.swift` | Window, glass sidebar, theme (gradient backdrop, glass panels, big round button, pill buttons, art slots) |
| `JunkPage.swift`, `PicksPage.swift`, `MaintenancePages.swift`, `SpaceLens.swift`, `Pages.swift` | System Junk; Installers, Project Junk, Slim Apps and the Uninstaller; Optimization and Touch ID; Space Lens; Dashboard, History, Settings |
| `windows/src/ui/` | Windows app: window and input (`mod.rs`), painting (`gfx.rs`), pages (`pages.rs`), engine jobs on threads (`jobs.rs`) |
| `shared/tools.json` | Sidebar tools: title, group, colors, icon, points (how a tool looks; what it does is in the engine) |
| `Support.swift` | The JSON reader for engine replies, `Tools`, and the on-disk cache of earlier results |
| `macos/Resources/<lang>.lproj` | Translations. Keys are the English text |
| `design/`, `macos/Resources/Art/` | Art and icon masters, and the optimized copies the apps ship; see Art below |

## Engine

**What may be removed** is decided in one place (`guard.rs`) for every tool, at scan time and again right before removal:

- only inside the user's home (an app bundle in `/Applications` for the Uninstaller), never personal folders (Documents, Desktop, Pictures, iCloud Drive, Keychains, Mail, Messages, `.ssh`…), never a kept folder itself (`~/Library/Caches`, `AppData\Local`…);
- inside the folder the rule or finder found it in, with no symlink anywhere on the way;
- not while the app that owns it is running;
- still the same file system object (device, inode, type) that was measured, or it is left alone;
- each tool opens only what it needs: the Uninstaller an app's preferences, Installers installer files in Downloads, Project Junk known build folders next to their project file.

Caches are deleted; apps, installers and slimmed originals go to the Trash (put back from Finder). Every attempt is written to `history.jsonl` in the app's support folder. The safety tests are in `engine/tests/clean.rs` and `engine/tests/picks.rs`.

**Adding a cleaning target:** add a `[[rule]]` to `engine/rules/macos.toml` (or `windows.toml`) with `id`, `group`, `title`, `paths` (patterns with `~`, `%VAR%`, `*`), and as needed `target = "self"`, `include`/`exclude` name patterns, `min_age_days`, `running` (owning apps), `item_is_bundle_id`, `default = false` for things worth a look first, and a `note`. More specific rules go before general ones. Add the title and note to `vi.lproj`. `cargo test` checks every shipped rule.

### Speed

Measured on an Apple M2 Mac mini (16 GB, macOS 27.2, about 450 GB used, load average 5–6), the engine on its own (`oce run '<request>'`), reading only, median of 3 runs:

| Task | First run | Again | Peak RAM |
|---|---:|---:|---:|
| System Junk scan | 6.5 s | 0.1 s (shown at once; exact again after 4.7 s) | 25 MiB |
| Space Lens: ~/Library | 5.0 s | 0.20 s | 36 MiB |
| Space Lens: startup disk | 20.5 s | 1.1 s | 144 MiB |
| App list with sizes | 2.0 s | 0.05 s | 11 MiB |
| Project Junk search | 4.4 s |  | 10 MiB |
| Installers | 0.08 s |  | 6 MiB |
| Dashboard reading | 0.38 s |  | 7 MiB |

"Again" reuses what has not changed: System Junk shows the last sizes at once (only for display; what is removed and the bytes freed come from the new measurement); Space Lens asks macOS (FSEvents) which folders changed since the last scan and reads only those; app sizes are kept until an app changes.

The app itself, idle (window open on the Dashboard or System Junk, or closed with the menu bar item): about 0.1% CPU and 45 MB of memory.

## Translations

Write UI text in English in code. `./build.sh` lists every string missing from each non-English `Localizable.strings`. To add a language, create `macos/Resources/<code>.lproj/Localizable.strings` and add the code to `CFBundleLocalizations` in `build.sh`.

## Art

Every page works without art: a missing illustration falls back to an SF Symbol on a glass disc. Masters live in `design/` (not bundled): `design/art/<module id>.png` (1024×1024, transparent background, glass 3D style) and `design/AppIcon.png` (1024×1024, full bleed). `python3 design/export.py [art|icons]` (needs Pillow and numpy) writes what ships and is committed; drop a new master in and rerun:

- `macos/Resources/Art/<id>.heic`: margins trimmed so every subject fills ~86% of the frame, 520 px (largest use is 260 pt @2x), HEIC with alpha;
- `macos/Resources/AppIcon.icns`: squircle on Apple's 824/1024 grid with a drop shadow; 16–64 px use `design/AppIcon-small.png` if present (a bolder, simpler master), else a white broom drawn by the script on a darker tile;
- `windows/app.ico`: the same, with a tighter margin, 16–256 px.

Art ids: `dashboard`, `spaceLens`, `uninstaller`, `history`, `scanning`, `done`, `volumeInternal`, `volumeExternal`, and every tool id in `shared/tools.json`. Also `privacy`, `schedule` and `empty`.

## License

[MIT](LICENSE): use, change, share and sell it as you like; keep the copyright notice.
The engine's code and rules were written for OpenCleaner (nothing GPL-licensed is bundled), and its Rust dependencies are MIT or Apache-2.0.
