#!/usr/bin/env bash
# Builds a drag-to-install disk image: Rustsheet.app plus a link to
# /Applications, compressed (UDZO) and ad-hoc signed.
#
#   scripts/make-dmg.sh                 # dist/Rustsheet-<versie>.dmg, plus een kopie in ~/Downloads
#   scripts/make-dmg.sh --out <map>     # kopie naar een andere map
#   scripts/make-dmg.sh --no-copy       # alleen dist/
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ARCH="$(uname -m)"
NAME="Rustsheet-${VERSION}-${ARCH}"
DMG="$ROOT/dist/$NAME.dmg"
OUT="$HOME/Downloads"
COPY=1

while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --no-copy) COPY=0; shift ;;
    *) echo "onbekende optie: $1" >&2; exit 2 ;;
  esac
done

scripts/bundle.sh >/dev/null

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/Rustsheet"
# ditto keeps the bundle's extended attributes and code signature intact.
ditto dist/Rustsheet.app "$STAGE/Rustsheet/Rustsheet.app"
ln -s /Applications "$STAGE/Rustsheet/Applications"

rm -f "$DMG"
hdiutil create -quiet -volname "Rustsheet $VERSION" -srcfolder "$STAGE/Rustsheet" -fs HFS+ -format UDZO -imagekey zlib-level=9 -ov "$DMG"
codesign --force --sign - --timestamp=none "$DMG" >/dev/null 2>&1 || echo "let op: DMG niet ondertekend" >&2
hdiutil verify -quiet "$DMG"

if [ "$COPY" = 1 ]; then
  mkdir -p "$OUT"
  cp -f "$DMG" "$OUT/"
  echo "$OUT/$NAME.dmg"
else
  echo "$DMG"
fi
