#!/bin/zsh
# Builds the engine as the static library the app links: engine/target/oc/liboc_engine.a
# Usage: ./build-lib.sh [--debug] [--universal]   (--universal: arm64 + x86_64, what CI ships)
set -euo pipefail
cd "${0:A:h}"

PROFILE=release
FLAGS=(--release --lib)
TARGETS=()
for arg in "$@"; do
  case $arg in
    --debug) PROFILE=debug; FLAGS=(--lib) ;;
    --universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

mkdir -p target/oc
if (( ${#TARGETS} )); then
  LIBS=()
  for t in $TARGETS; do
    cargo build $FLAGS --target $t >&2
    LIBS+=(target/$t/$PROFILE/liboc_engine.a)
  done
  lipo -create $LIBS -output target/oc/liboc_engine.a
else
  cargo build $FLAGS >&2
  cp target/$PROFILE/liboc_engine.a target/oc/liboc_engine.a
fi
echo target/oc/liboc_engine.a
