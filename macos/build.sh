#!/bin/zsh
# Builds macos/build/OpenCleaner.app.  Usage: ./build.sh [--debug] [--universal]
#   --universal  arm64 + x86_64 in one binary (what CI ships); default is this Mac's arch only
set -euo pipefail
cd "${0:A:h}"

CONFIG=release
ARCHS=()
ENGINE_FLAGS=()
for arg in "$@"; do
  case $arg in
    --debug) CONFIG=debug; ENGINE_FLAGS+=(--debug) ;;
    --universal) ARCHS=(--arch arm64 --arch x86_64); ENGINE_FLAGS+=(--universal) ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done
VERSION=$(cat ../VERSION)
# Builds made outside CI are "OpenCleaner Dev" with their own bundle id: macOS would
# otherwise open whichever copy it saw last when the user starts the installed app.
BUNDLE_ID=io.github.opencleaner
NAME=OpenCleaner
if [[ -z "${CI:-}" ]]; then BUNDLE_ID+=.dev; NAME="OpenCleaner Dev"; fi
STRINGS=$(mktemp -d)

../engine/build-lib.sh "${ENGINE_FLAGS[@]}" >/dev/null  # the Rust engine, linked into the app

swift build -c $CONFIG "${ARCHS[@]}" \
  -Xswiftc -emit-localized-strings -Xswiftc -emit-localized-strings-path -Xswiftc "$STRINGS"
BIN=$(swift build -c $CONFIG "${ARCHS[@]}" --show-bin-path)/OpenCleaner

APP=build/OpenCleaner.app
rm -rf $APP
mkdir -p $APP/Contents/{MacOS,Resources}
cp "$BIN" $APP/Contents/MacOS/
cp ../shared/tools.json $APP/Contents/Resources/
cp -R Resources/*.lproj $APP/Contents/Resources/
[[ -d Resources/Art ]] && cp -R Resources/Art $APP/Contents/Resources/
cp Resources/MenuBarIcon.png Resources/MenuBarIcon@2x.png $APP/Contents/Resources/
ICON_KEY=""
if [[ -f Resources/AppIcon.icns ]]; then  # art and icon are generated from design/ by design/export.py
  cp Resources/AppIcon.icns $APP/Contents/Resources/
  ICON_KEY="<key>CFBundleIconFile</key><string>AppIcon</string>"
fi

cat > $APP/Contents/Info.plist <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key><string>OpenCleaner</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleLocalizations</key><array><string>en</string><string>vi</string></array>
  <key>LSMinimumSystemVersion</key><string>26.0</string>
  <key>NSHighResolutionCapable</key><true/>
  $ICON_KEY
</dict></plist>
EOF

codesign --force --sign "${SIGN_ID:--}" $APP
echo "→ $APP"

# every string the compiler marked localizable, plus tools.json texts, must exist in each language
python3 - "$STRINGS" Resources ../shared/tools.json ../engine/rules/macos.toml <<'PY'
import glob, json, subprocess, sys, tomllib
strings, res, manifest, rules = sys.argv[1:]
keys = set()
for f in glob.glob(f"{strings}/*.stringsdata"):
    for table in json.load(open(f)).get("tables", {}).values():
        keys |= {e["key"] for e in table}
for t in json.load(open(manifest)).get("tools", []):
    keys |= {t[k] for k in ("title", "subtitle", "group") if t.get(k)}
    keys |= set(t.get("points", []))
keys |= {"Cleanup", "Speed", "Applications", "Files", "Security"}
# System Junk shows rule and group titles and notes through tr()
pack = tomllib.load(open(rules, "rb"))
keys |= {g["title"] for g in pack.get("group", [])}
keys |= {r[k] for r in pack.get("rule", []) for k in ("title", "note") if r.get(k)}
keys -= {"", "OpenCleaner"}
for lang in sorted(glob.glob(f"{res}/*.lproj")):
    if lang.endswith("en.lproj"):
        continue
    have = json.loads(subprocess.check_output(["plutil", "-convert", "json", "-o", "-", f"{lang}/Localizable.strings"]))
    for k in sorted(keys - set(have)):
        print(f"warning: {lang.split('/')[-1]} is missing {k!r}")
PY
