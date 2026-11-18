#!/bin/zsh
# Legacy engine (mo), kept only to compare against the Rust engine until it is retired.
# Builds it into macos/build/engine-<version>-<patches>/: the pinned mo release (VERSION),
# its source checked against source.sha256, with every patches/*.patch applied, and the Go helpers (analyze, status) compiled as
# universal binaries. Needs Go (the toolchain mo's go.mod names is fetched automatically).
# Re-running is a no-op while the version and patches are unchanged.
# Prints the engine directory; with --source it also writes the patched source tarball
# (GPL-3.0 §6) next to it and prints that path on a second line. --rebuild forces a
# fresh build; ENGINE_TEST=1 runs the analyzer's Go tests on the patched source first.
set -euo pipefail
cd "${0:A:h}"
HERE=$PWD
cd ../../macos

VERSION=$(cat $HERE/VERSION)
PATCHES=($HERE/patches/*.patch(N))
STAMP=$(cat $HERE/source.sha256 $PATCHES | shasum -a 256 | cut -c1-8)
OUT=build/engine-$VERSION-$STAMP
SRC_TAR=build/mo-engine-$VERSION-$STAMP-source.tar.gz

if [[ ! -x $OUT/mole || ${1:-} == --rebuild || ( ${1:-} == --source && ! -f $SRC_TAR ) ]]; then
  REPO=https://github.com/tw93/mole
  TMP=$(mktemp -d)
  trap 'rm -rf $TMP' EXIT

  curl -fsSL "$REPO/archive/refs/tags/$VERSION.tar.gz" -o $TMP/src.tar.gz
  echo "$(cat $HERE/source.sha256)  $TMP/src.tar.gz" | shasum -a 256 -c - >&2

  SRC=$TMP/mole-$VERSION
  mkdir -p $SRC
  tar -xzf $TMP/src.tar.gz -C $SRC --strip-components 1
  for patch in $PATCHES; do
    patch -d $SRC -p1 --forward --quiet < $patch >&2
  done

  [[ ${ENGINE_TEST:-} == 1 ]] && (cd $SRC && go test ./cmd/analyze) >&2

  rm -rf build/engine-*(N) build/mo-engine-*-source.tar.gz(N)
  mkdir -p $OUT/bin
  cp $SRC/mole $SRC/LICENSE $OUT/
  cp -R $SRC/lib $OUT/
  cp $SRC/bin/*.sh $OUT/bin/
  for tool in analyze status; do
    for arch in arm64 amd64; do
      (cd $SRC && CGO_ENABLED=0 GOOS=darwin GOARCH=$arch go build -trimpath -ldflags="-s -w" -o $TMP/$tool-$arch ./cmd/$tool) >&2
    done
    lipo -create $TMP/$tool-arm64 $TMP/$tool-amd64 -output $OUT/bin/$tool-go
  done
  chmod +x $OUT/mole $OUT/bin/*
  # `du` for mo's scripts: analyze-go answers du -sk[cPx] itself (patch 0002 puts it in PATH)
  mkdir -p $OUT/bin/fastdu && ln -s ../analyze-go $OUT/bin/fastdu/du
  {
    echo "Source: $REPO/tree/$VERSION (GPL-3.0)"
    (( ${#PATCHES} )) && echo "Modified by OpenCleaner: ${(j:, :)${(@)PATCHES:t}}. The patched source ships with each OpenCleaner release."
  } > $OUT/SOURCE

  # the exact source of what we ship, for the release page
  tar -czf $SRC_TAR -C $TMP mole-$VERSION
fi

echo $OUT
[[ ${1:-} == --source ]] && echo $SRC_TAR
exit 0
