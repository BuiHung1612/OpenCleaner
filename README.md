# OpenCleaner

Free, open-source Mac cleaner with a Liquid Glass UI. macOS first (SwiftUI), Windows next.
English is the default language; Vietnamese is included.

The cleaning engine is [`mo`](https://github.com/tw93/mole) (GPL-3.0), bundled inside the app, so users install nothing else. OpenCleaner ships it with a few performance patches (`engine/patches/`), see [Engine patches](#engine-patches). The app runs it as a separate process. OpenCleaner is an independent project, not affiliated with or endorsed by the Mole project or MacPaw.

## Build (macOS 26+)

```sh
cd macos
./build.sh            # -> macos/build/OpenCleaner.app   (--debug for a debug build)
                      #    builds the pinned, patched engine on first run (fetch-engine.sh, needs Go)
swift run             # quick dev run, English only, no art bundle
```

## CI and releases

`.github/workflows/opencleaner.yml` builds a universal (Apple Silicon + Intel) app on every push and uploads `OpenCleaner-macos.zip` as a workflow artifact.

- **Release:** bump `VERSION`, commit, then push a matching tag: `git tag v0.1.0 && git push origin v0.1.0`. CI checks the tag matches `VERSION`, then creates a GitHub Release with the app zip and the patched source of the bundled mo engine (GPL-3.0).
- **Signing (optional, recommended):** run `macos/make-signing-cert.sh` once to create the fixed self-signed identity "OpenCleaner Code Signing", then `macos/make-signing-cert.sh --secrets OWNER/REPO` to upload `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD` and `MACOS_SIGN_ID`. A fixed signature keeps Full Disk Access granted across updates; ad-hoc builds lose it every release. It does not remove the first-launch Gatekeeper prompt (right-click → Open); that needs a Developer ID and notarization, not set up yet. Back up `~/.opencleaner-signing`: losing it means users grant access again once.

## Layout

| Path | What |
|---|---|
| `ENGINE_VERSION` | mo release bundled in the app (a GitHub tag, e.g. `V1.72.0`) |
| `engine/source.sha256` | Checksum of that release's source tarball |
| `engine/patches/` | OpenCleaner's changes to mo, applied in name order |
| `macos/fetch-engine.sh` | Downloads that release's source, checks it, applies the patches, builds `build/engine-<version>-<hash>/` (mole + lib + bin, universal Go helpers) and, with `--source`, the patched source tarball |
| `shared/cli.json` | **Every `mo` command line the app uses**, plus the sidebar tools (title, group, colors, icon). Shared by every platform. |
| `macos/Sources/OpenCleaner/CLI.swift` | Runs `mo`: finds the binary, reads JSON output, streams live output through a pseudo-terminal so password / y-n prompts work |
| `macos/Sources/OpenCleaner/App.swift` | Window, glass sidebar, theme (gradient backdrop, glass panels, big round button, pill buttons, art slots) |
| `ToolPage.swift` | Any tool from `cli.json`: intro → Scan (dry run) → review → Run |
| `SpaceLens.swift` | Disk map: volumes, folder list + bubble chart, move to Trash |
| `Pages.swift` | Dashboard, Uninstaller, History, Settings |
| `macos/Resources/<lang>.lproj` | Translations. Keys are the English text |
| `macos/Resources/Art/` | Optional illustrations, see below |

## When `mo` changes

**New mo release:** put the new tag in `ENGINE_VERSION` and its source checksum in `engine/source.sha256` (`curl -fsSL https://github.com/tw93/mole/archive/refs/tags/<tag>.tar.gz | shasum -a 256`), run `ENGINE_TEST=1 ./fetch-engine.sh --rebuild` (re-export any patch that no longer applies), then `./build.sh`, check that the pages still work, then ship. Nothing else to edit unless the commands changed.

The app never builds a command line in Swift code:

- **A flag or subcommand was renamed:** edit `shared/cli.json`. To fix an installed app without rebuilding: Settings → *Command list → Customize…* copies the file to `~/Library/Application Support/OpenCleaner/cli.json`, which takes priority. Reopen the app after editing it.
- **A new tool:** add an entry to `tools` in `cli.json` (`id`, `group`, `title`, `subtitle`, `points`, `icon` (an SF Symbol), `colors`, `preview`, `run`, plus `terminal: true` when it needs arrow-key menus). It appears in the sidebar on its own. Add its strings to `vi.lproj` (`build.sh` warns about missing ones).
- **JSON output gained or lost fields:** nothing breaks. Output is read with a schema-less `JSON` wrapper, so missing fields show "—".

## Translations

Write UI text in English in code. `./build.sh` lists every string missing from each non-English `Localizable.strings`. To add a language, create `macos/Resources/<code>.lproj/Localizable.strings` and add the code to `CFBundleLocalizations` in `build.sh`.

## Art

Every page works without art: a missing illustration falls back to an SF Symbol on a glass disc. Drop PNGs (1024×1024, transparent background) into `macos/Resources/Art/` named after the module id, then rebuild:

`dashboard`, `spaceLens`, `uninstaller`, `history`, `clean`, `installer`, `purge`, `optimize`, `slim`, `touchid`.
The app icon is `macos/Resources/AppIcon.png` (1024×1024); `build.sh` turns it into `.icns`.

## Engine patches

`engine/patches/*.patch` are applied on top of the upstream release. Each one keeps mo's own tests passing and adds its own.

- `0001-analyzer-parallel-sizing.patch`: the analyzer (`analyze-go`) measures folders with a parallel, in-process walk built on `getattrlistbulk(2)` instead of `/usr/bin/du`, and reads directory listings with that same call instead of one `lstat` per file. Sizes match `du -skPx` to the KB. Space Lens: the full-disk overview went from 30 s (cut off by a timeout, about 70 GB short) to about 20 s with complete numbers, `~/workspace` from 24 s to 6 s. `--json` now always measures instead of reusing caches up to a week old; the app shows its own previous result while it runs. The same binary also answers `du -sk[cPx]` when run as `du`, and `--freed-kb` (what deleting a tree frees, as clean.sh counts it).
- `0002-scripts-use-fast-sizes.patch`: mo's scripts use that `du` (`bin/fastdu/du`, first in PATH only when `du` would otherwise be `/usr/bin/du`) and `--freed-kb` instead of their Perl walk. `clean --dry-run`: same result, about 5% faster (78 s to 75 s); its remaining time is shell logic, not measuring.

Both keep the full upstream test suite at the same result as unpatched mo (Go tests, and all 3009 bats tests: identical pass/fail lists on this machine) and add their own Go tests against `du`, `lstat` and the original Perl rule.

To change a patch: unpack the source tarball `fetch-engine.sh --source` writes, `git init` it, apply the patches, edit, and `git diff` the result back into `engine/patches/`.

## License

The app's own license is TBD. The bundled engine is GPL-3.0 and modified: `engine/LICENSE` and `engine/SOURCE` (upstream tag plus the patch list) ship inside the app, and every release attaches the exact patched source (`mo-engine-<version>-<hash>-source.tar.gz`), as GPL-3.0 §6 asks.
