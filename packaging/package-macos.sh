#!/bin/bash
# Build Genatrix.app and a disk image (design 09, "分发" and "包里的样子").
#
#   packaging/package-macos.sh
#
# Environment, all optional:
#   GENATRIX_TELEGRAM_API_ID, GENATRIX_TELEGRAM_API_HASH
#       Genatrix's own Telegram application credentials, compiled in so that
#       nobody who installs it is asked for developer credentials (design 05).
#       Without them the build works and Settings says Telegram is unavailable.
#   GENATRIX_SIGN_IDENTITY
#       A "Developer ID Application: ..." identity in the keychain. Without it
#       the app is signed ad hoc: it runs, but the first open needs a click in
#       System Settings > Privacy & Security.
#   GENATRIX_NOTARY_PROFILE
#       A notarytool keychain profile (xcrun notarytool store-credentials).
#       With it and an identity, the disk image is notarized and stapled.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/dist"
APP="$OUT/Genatrix.app"
IDENTITY="${GENATRIX_SIGN_IDENTITY:--}"
say() { printf '\n== %s\n' "$*"; }

if [ "$(uname -m)" != "arm64" ]; then
  echo "Genatrix is built for Apple silicon; run this on an arm64 Mac." >&2
  exit 1
fi
if [ -z "${GENATRIX_TELEGRAM_API_ID:-}" ] || [ -z "${GENATRIX_TELEGRAM_API_HASH:-}" ]; then
  echo "note: no Telegram application credentials in the environment; this build cannot add Telegram."
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
BUILD="$(git -C "$ROOT" rev-list --count HEAD)"
COMMIT="$(git -C "$ROOT" rev-parse --short HEAD)"

say "building the core, the gateway, inference and the connectors ($VERSION, build $BUILD)"
( cd "$ROOT" && cargo build --release --workspace )
say "building the menu bar shell"
swift build -c release --package-path "$ROOT/apps/menubar"

say "assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$APP/Contents/Library/LaunchAgents"
cp "$ROOT/apps/menubar/.build/release/genatrix-menubar" "$APP/Contents/MacOS/"
for bin in genatrix genatrix-llm genatrix-infer genatrix-imap genatrix-telegram; do
  cp "$ROOT/target/release/$bin" "$APP/Contents/MacOS/"
done

# MLX wants mlx.metallib beside the executable and writes one there if it is
# missing, which inside a signed bundle would break the signature. The file
# lives in Resources, where the rules want it, and MacOS holds a relative link.
METALLIB="$(ls -t "$ROOT"/target/release/build/crabllm-mlx-*/out/default.metallib 2>/dev/null | head -1 || true)"
[ -n "$METALLIB" ] || METALLIB="$ROOT/target/release/mlx.metallib"
cp "$METALLIB" "$APP/Contents/Resources/mlx.metallib"
ln -s ../Resources/mlx.metallib "$APP/Contents/MacOS/mlx.metallib"

cp "$ROOT/packaging/xyz.dpt.genatrix.app.plist" "$APP/Contents/Library/LaunchAgents/"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>xyz.dpt.genatrix</string>
  <key>CFBundleName</key><string>Genatrix</string>
  <key>CFBundleDisplayName</key><string>Genatrix</string>
  <key>CFBundleExecutable</key><string>genatrix-menubar</string>
  <key>CFBundleIconFile</key><string>Genatrix</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundleVersion</key><string>$BUILD</string>
  <key>GenatrixCommit</key><string>$COMMIT</string>
  <key>LSMinimumSystemVersion</key><string>15.0</string>
  <key>LSRequiresNativeExecution</key><true/>
  <key>LSArchitecturePriority</key><array><string>arm64</string></array>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSAppTransportSecurity</key>
  <dict><key>NSAllowsLocalNetworking</key><true/></dict>
  <key>NSHumanReadableCopyright</key><string>Genatrix runs on this Mac only.</string>
</dict>
</plist>
PLIST

say "drawing the icon"
ICONWORK="$(mktemp -d)"
qlmanage -t -s 1024 -o "$ICONWORK" "$ROOT/web/public/icon.svg" >/dev/null 2>&1 || true
PNG="$ICONWORK/icon.svg.png"
if [ -f "$PNG" ]; then
  mkdir -p "$ICONWORK/Genatrix.iconset"
  for size in 16 32 64 128 256 512; do
    sips -z $size $size "$PNG" --out "$ICONWORK/Genatrix.iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z $double $double "$PNG" --out "$ICONWORK/Genatrix.iconset/icon_${size}x${size}@2x.png" >/dev/null
  done
  iconutil -c icns "$ICONWORK/Genatrix.iconset" -o "$APP/Contents/Resources/Genatrix.icns"
else
  echo "note: could not render the icon; the app will show the generic one."
fi
rm -rf "$ICONWORK"

say "signing ($([ "$IDENTITY" = "-" ] && echo 'ad hoc' || echo "$IDENTITY"))"
SIGN=(codesign --force --sign "$IDENTITY" --entitlements "$ROOT/packaging/entitlements.plist")
if [ "$IDENTITY" != "-" ]; then SIGN+=(--options runtime --timestamp); fi
for bin in genatrix genatrix-llm genatrix-infer genatrix-imap genatrix-telegram; do
  "${SIGN[@]}" --identifier "xyz.dpt.genatrix.$bin" "$APP/Contents/MacOS/$bin"
done
"${SIGN[@]}" "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"

say "making the disk image"
STAGE="$(mktemp -d)"
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG="$OUT/Genatrix-$VERSION-$BUILD.dmg"
rm -f "$DMG"
hdiutil create -volname "Genatrix" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
rm -rf "$STAGE"
if [ "$IDENTITY" != "-" ]; then
  codesign --force --sign "$IDENTITY" --timestamp "$DMG"
  if [ -n "${GENATRIX_NOTARY_PROFILE:-}" ]; then
    say "notarizing (this takes a few minutes)"
    xcrun notarytool submit "$DMG" --keychain-profile "$GENATRIX_NOTARY_PROFILE" --wait
    xcrun stapler staple "$DMG"
  fi
fi

say "done"
du -sh "$APP" "$DMG" | sed 's/^/  /'
echo "  open the disk image, drag Genatrix into Applications, open it from there."
if [ "$IDENTITY" = "-" ]; then
  echo "  signed ad hoc: on another Mac the first open is refused once; allow it in"
  echo "  System Settings > Privacy & Security, or right-click the app and choose Open."
fi
