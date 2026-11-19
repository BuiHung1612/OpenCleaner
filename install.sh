#!/bin/sh
# Installs (or updates) OpenCleaner on macOS from the latest GitHub release:
#
#   curl -fsSL https://raw.githubusercontent.com/BuiHung1612/OpenCleaner/main/install.sh | sh
#
# It downloads OpenCleaner.dmg, checks the app's code signature, and copies it to
# /Applications (an existing copy goes to the Trash). Files fetched with curl are not
# marked "downloaded from the Internet", so macOS opens the app without the
# "could not verify" prompt a browser download gets (the app is not notarized).
set -eu

REPO="BuiHung1612/OpenCleaner"
URL="https://github.com/$REPO/releases/latest/download/OpenCleaner.dmg"
DEST="${OPENCLEANER_DIR:-/Applications}"
APP="$DEST/OpenCleaner.app"

[ "$(uname -s)" = Darwin ] || { echo "OpenCleaner for macOS only; on Windows download OpenCleaner.exe from https://github.com/$REPO/releases" >&2; exit 1; }

TMP=$(mktemp -d)
MOUNT="$TMP/mount"
cleanup() {
    hdiutil detach -quiet "$MOUNT" 2>/dev/null || true
    rm -rf "$TMP"
}
trap cleanup EXIT INT TERM

echo "Downloading OpenCleaner…"
curl -fL --progress-bar -o "$TMP/OpenCleaner.dmg" "$URL"
hdiutil attach -quiet -nobrowse -readonly -noautoopen -mountpoint "$MOUNT" "$TMP/OpenCleaner.dmg"

NEW="$MOUNT/OpenCleaner.app"
[ -d "$NEW" ] || { echo "The download has no OpenCleaner.app." >&2; exit 1; }
codesign --verify --deep --strict "$NEW" || { echo "The app's signature is broken; nothing was installed." >&2; exit 1; }
VERSION=$(defaults read "$NEW/Contents/Info.plist" CFBundleShortVersionString)

# a running copy is quit first; an old copy goes to the Trash, not deleted
osascript -e 'tell application id "io.github.opencleaner" to quit' >/dev/null 2>&1 || true
if [ -e "$APP" ]; then
    osascript -e "tell application \"Finder\" to delete POSIX file \"$APP\"" >/dev/null 2>&1 || rm -rf "$APP"
fi
mkdir -p "$DEST"
ditto "$NEW" "$APP"

echo "OpenCleaner $VERSION is in $DEST."
[ -n "${OPENCLEANER_DIR:-}" ] || open "$APP"
