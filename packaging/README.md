# Package manifests

Templates for Homebrew and winget. Each tagged release renders them (version and sha256 filled
in) and attaches them to the GitHub Release: `opencleaner.rb` and three `BuiHung1612.OpenCleaner*.yaml`.
Nothing is submitted automatically; do it by hand after the release is out.

## Homebrew (own tap)

The macOS app is not notarized (no Apple Developer account), so it would not be accepted into
homebrew/cask. Use your own tap instead:

1. Once: create the public repo `BuiHung1612/homebrew-tap` with a `Casks/` folder.
2. Each release: download `opencleaner.rb` from the release and commit it as `Casks/opencleaner.rb`.
3. Users install with `brew install --cask buihung1612/tap/opencleaner`. On first launch macOS
   still asks them to allow the app (System Settings > Privacy & Security > Open Anyway).

The app updates itself (`auto_updates true`), so `brew upgrade` skips it unless run with `--greedy`.

## winget (PR to microsoft/winget-pkgs)

1. Set a real `License` in `winget/BuiHung1612.OpenCleaner.locale.en-US.yaml` first (it says TBD).
2. Download the three `.yaml` files from the release and check them: `winget validate --manifest <folder>`,
   and on a test machine `winget install --manifest <folder>`.
3. Fork microsoft/winget-pkgs, put them in `manifests/b/BuiHung1612/OpenCleaner/<version>/`, and
   open a PR. (`wingetcreate submit <folder>` does the fork and PR for you.)

The exe is not code-signed, so SmartScreen may warn on first run and the winget review can take
longer; that is expected.

## Microsoft Store (MSIX)

Every CI run builds `OpenCleaner.msix` (artifact `OpenCleaner-msix`) from `msix/AppxManifest.xml`:
the same exe, unsigned (the Store signs it, so Smart App Control lets it run). Product
`slowbuildstudio.OpenCleaner`, Store ID `9PCVNWSXKCCZ`.

1. First submission by hand in Partner Center: upload the `.msix` from a tag's CI run, fill in
   the listing, age rating and privacy policy, and explain the restricted capabilities:
   `runFullTrust` (a desktop app) and `unvirtualizedResources` (a cleaner must remove the real
   caches in AppData and turn off startup entries in HKCU, not a per-package copy).
2. After that, each tag's `store` job submits the new package, once these repo secrets are set
   (Partner Center › Account settings › User management › Microsoft Entra applications, role
   Manager): `MSSTORE_TENANT_ID`, `MSSTORE_CLIENT_ID`, `MSSTORE_CLIENT_SECRET`, `MSSTORE_SELLER_ID`.
   Without them the job does nothing.

The Store copy never updates itself (it sees it runs from `WindowsApps`); the Store does it.
