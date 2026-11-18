# OpenCleaner

Free, open-source Mac cleaner with a Liquid Glass UI. macOS first (SwiftUI), Windows next.
English is the default language; Vietnamese is included.

The engine is OpenCleaner's own, written in Rust (`engine/`) and linked into the app as a static library: no helper processes, one code base for macOS and Windows. It replaced [`mo`](https://github.com/tw93/mole), which earlier versions bundled. OpenCleaner is an independent project, not affiliated with or endorsed by the Mole project or MacPaw.

## Build (macOS 26+)

```sh
cd macos
./build.sh            # -> macos/build/OpenCleaner.app   (--debug, --universal)
                      #    builds the Rust engine library first (engine/build-lib.sh, needs cargo)
../engine/build-lib.sh && swift run   # quick dev run (rebuild the library after engine changes)
cd ../engine && cargo test             # engine tests (also run in CI on macOS and Windows)
```

## CI and releases

`.github/workflows/opencleaner.yml` tests the engine on macOS and Windows, builds a universal (Apple Silicon + Intel) app on every push and uploads `OpenCleaner-macos.zip` as a workflow artifact.

- **Release:** bump `VERSION`, commit, then push a matching tag: `git tag v0.1.0 && git push origin v0.1.0`. CI checks the tag matches `VERSION`, then creates a GitHub Release with the app zip.
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
| `shared/tools.json` | Sidebar tools: title, group, colors, icon, points (how a tool looks; what it does is in the engine) |
| `Support.swift` | The JSON reader for engine replies, `Tools`, and the on-disk cache of earlier results |
| `macos/Resources/<lang>.lproj` | Translations. Keys are the English text |
| `macos/Resources/Art/` | Optional illustrations, see below |

## Engine

**What may be removed** is decided in one place (`guard.rs`) for every tool, at scan time and again right before removal:

- only inside the user's home (an app bundle in `/Applications` for the Uninstaller), never personal folders (Documents, Desktop, Pictures, iCloud Drive, Keychains, Mail, Messages, `.ssh`…), never a kept folder itself (`~/Library/Caches`, `AppData\Local`…);
- inside the folder the rule or finder found it in, with no symlink anywhere on the way;
- not while the app that owns it is running;
- still the same file system object (device, inode, type) that was measured, or it is left alone;
- each tool opens only what it needs: the Uninstaller an app's preferences, Installers installer files in Downloads, Project Junk known build folders next to their project file.

Caches are deleted; apps, installers and slimmed originals go to the Trash (put back from Finder). Every attempt is written to `history.jsonl` in the app's support folder. The safety tests are in `engine/tests/clean.rs` and `engine/tests/picks.rs`.

**Adding a cleaning target:** add a `[[rule]]` to `engine/rules/macos.toml` (or `windows.toml`) with `id`, `group`, `title`, `paths` (patterns with `~`, `%VAR%`, `*`), and as needed `target = "self"`, `include`/`exclude` name patterns, `min_age_days`, `running` (owning apps), `item_is_bundle_id`, `default = false` for things worth a look first, and a `note`. More specific rules go before general ones. Add the title and note to `vi.lproj`. `cargo test` checks every shipped rule.

### Rust engine vs mo

Measured before mo was removed, with `bench/compare.sh` (read-only; at commit `ebbed48`) on an Apple M2 Mac mini, 8 cores, load average about 5, best of 2 runs:

| Task | mo (legacy) | Rust engine | Speed-up | Peak RAM mo → Rust |
|---|---:|---:|---:|---:|
| System Junk preview | 77.8 s | 8.1 s | 9.6× | 159 → 19 MiB |
| Space Lens: startup disk | 35.5 s | 17.2 s | 2.1× | 83 → 22 MiB |
| Space Lens: ~/Library | 11.9 s | 4.2 s | 2.8× | 31 → 19 MiB |
| Space Lens: /Applications | 3.6 s | 1.4 s | 2.6× | 23 → 11 MiB |
| App list with sizes | 1.9 s | 1.2 s | 1.5× | 14 → 12 MiB |
| Dashboard reading | 0.85 s | 0.27 s | 3.1× | 23 → 7 MiB |
| Installers | 0.62 s | 0.01 s | 62× | 6 → 6 MiB |

Since then: System Junk preview **3.2 s** (private sizes are looked up only for files whose flags say they can differ; same results), and the Uninstaller's app sizes are remembered until an app changes (**0.02 s** on a repeat).

What each finds is not identical: System Junk rules differ (mo also cleans some system areas with sudo; the Rust rules add developer caches and skip apps that are open), and mo's Space Lens overview counts nested folders twice. The Dashboard figure is a fresh process (it samples CPU for 250 ms); in the app later readings take milliseconds. Directory walks are bound by the kernel, so they gain least.

## Translations

Write UI text in English in code. `./build.sh` lists every string missing from each non-English `Localizable.strings`. To add a language, create `macos/Resources/<code>.lproj/Localizable.strings` and add the code to `CFBundleLocalizations` in `build.sh`.

## Art

Every page works without art: a missing illustration falls back to an SF Symbol on a glass disc. Drop PNGs (1024×1024, transparent background) into `macos/Resources/Art/` named after the module id, then rebuild:

`dashboard`, `spaceLens`, `uninstaller`, `history`, `clean`, `installer`, `purge`, `optimize`, `slim`, `touchid`.
The app icon is `macos/Resources/AppIcon.png` (1024×1024); `build.sh` turns it into `.icns`.

## License

The app and its engine: license TBD. The engine's code and rules were written for OpenCleaner without copying mo's code or rule text (some behaviour, such as how freed bytes are counted, was matched to mo on purpose and checked against it), so the license is OpenCleaner's to choose; have it reviewed before relying on that. Nothing GPL-licensed is bundled any more.
