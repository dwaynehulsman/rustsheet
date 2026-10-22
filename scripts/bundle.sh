#!/usr/bin/env bash
# Builds Rustsheet.app in dist/: release binary, Info.plist and icon.
#
#   scripts/bundle.sh            # build dist/Rustsheet.app
#   scripts/bundle.sh --open     # build and open it
#   scripts/bundle.sh --icon     # also redraw assets/AppIcon.icns first
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
APP="$ROOT/dist/Rustsheet.app"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
BUNDLE_ID="nl.infodatek.rustsheet"

OPEN=0
for arg in "$@"; do
  case "$arg" in
    --open) OPEN=1 ;;
    --icon)
      ICONSET="$(mktemp -d)/AppIcon.iconset"
      cargo run --quiet --release -p rustsheet-icon -- "$ICONSET"
      iconutil -c icns "$ICONSET" -o assets/AppIcon.icns
      ;;
    *) echo "onbekende optie: $arg" >&2; exit 2 ;;
  esac
done

cargo build --release -p rustsheet

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/rustsheet "$APP/Contents/MacOS/rustsheet"
cp assets/AppIcon.icns "$APP/Contents/Resources/AppIcon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Rustsheet</string>
  <key>CFBundleDisplayName</key><string>Rustsheet</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleVersion</key><string>${VERSION}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>rustsheet</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleDevelopmentRegion</key><string>nl</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>© Infodatek</string>
  <key>CFBundleDocumentTypes</key>
  <array>
    <dict>
      <key>CFBundleTypeName</key><string>Excel-werkmap</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Default</string>
      <key>LSItemContentTypes</key>
      <array><string>org.openxmlformats.spreadsheetml.sheet</string></array>
      <key>CFBundleTypeExtensions</key><array><string>xlsx</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>CSV-tekst</string>
      <key>CFBundleTypeRole</key><string>Editor</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>public.comma-separated-values-text</string>
        <string>public.tab-separated-values-text</string>
      </array>
      <key>CFBundleTypeExtensions</key><array><string>csv</string><string>tsv</string></array>
    </dict>
    <dict>
      <key>CFBundleTypeName</key><string>Andere werkmap</string>
      <key>CFBundleTypeRole</key><string>Viewer</string>
      <key>LSHandlerRank</key><string>Alternate</string>
      <key>LSItemContentTypes</key>
      <array>
        <string>com.microsoft.excel.xls</string>
        <string>org.openxmlformats.spreadsheetml.sheet.macroenabled</string>
        <string>com.microsoft.excel.sheet.binary.macroenabled</string>
        <string>org.oasis-open.opendocument.spreadsheet</string>
      </array>
      <key>CFBundleTypeExtensions</key><array><string>xls</string><string>xlsm</string><string>xlsb</string><string>ods</string></array>
    </dict>
  </array>
</dict>
</plist>
PLIST
printf 'APPL????' > "$APP/Contents/PkgInfo"

# Ad-hoc signature so Gatekeeper and the hardened runtime accept a local build.
codesign --force --sign - --timestamp=none "$APP" >/dev/null 2>&1 || echo "let op: ondertekenen mislukt, de app werkt lokaal nog wel" >&2


echo "$APP"
if [ "$OPEN" = 1 ]; then
  open "$APP"
fi
