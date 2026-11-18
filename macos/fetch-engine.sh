#!/bin/zsh
# Downloads the pinned mo release (../ENGINE_VERSION) into build/engine-<version>/ as a
# self-contained, relocatable engine: mole + lib/ + bin/ with universal Go helpers.
# Re-running is a no-op once that version is there. To move to a new mo release, change
# ENGINE_VERSION and rebuild.
set -euo pipefail
cd "${0:A:h}"

VERSION=$(cat ../ENGINE_VERSION)
OUT=build/engine-$VERSION
[[ -x $OUT/mole ]] && { echo $OUT; exit 0; }

REPO=https://github.com/tw93/mole
TMP=$(mktemp -d)
trap 'rm -rf $TMP' EXIT

curl -fsSL "$REPO/archive/refs/tags/$VERSION.tar.gz" -o $TMP/src.tar.gz
for arch in arm64 amd64; do
  curl -fsSL "$REPO/releases/download/$VERSION/binaries-darwin-$arch.tar.gz" -o $TMP/binaries-darwin-$arch.tar.gz
done
curl -fsSL "$REPO/releases/download/$VERSION/SHA256SUMS" -o $TMP/SHA256SUMS
(cd $TMP && grep -E ' binaries-darwin-(arm64|amd64)\.tar\.gz$' SHA256SUMS | shasum -a 256 -c - >&2)

mkdir -p $TMP/src $TMP/arm64 $TMP/amd64
tar -xzf $TMP/src.tar.gz -C $TMP/src --strip-components 1
tar -xzf $TMP/binaries-darwin-arm64.tar.gz -C $TMP/arm64
tar -xzf $TMP/binaries-darwin-amd64.tar.gz -C $TMP/amd64

rm -rf $OUT && mkdir -p $OUT/bin
cp $TMP/src/mole $TMP/src/LICENSE $OUT/
cp -R $TMP/src/lib $OUT/
cp $TMP/src/bin/*.sh $OUT/bin/
for tool in analyze status; do
  arm=$(find $TMP/arm64 -type f -name "$tool*" | head -1)
  amd=$(find $TMP/amd64 -type f -name "$tool*" | head -1)
  lipo -create "$arm" "$amd" -output $OUT/bin/$tool-go
done
chmod +x $OUT/mole $OUT/bin/*
echo "Source: $REPO/tree/$VERSION (GPL-3.0, unmodified)" > $OUT/SOURCE
echo $OUT
