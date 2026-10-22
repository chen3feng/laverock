#!/usr/bin/env bash
# Build and launch the desktop shell.
#
# On macOS the binary is wrapped in a minimal .app bundle: microphone
# access (getUserMedia) needs a bundle with NSMicrophoneUsageDescription
# and its own TCC identity — a bare executable launched from a shell is
# attributed to the terminal and usually denied silently.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

cargo build -p laverock-desktop -p laverock-server

if [ "$(uname)" != "Darwin" ]; then
  exec "$ROOT/target/debug/laverock-desktop"
fi

APP="$ROOT/target/debug/Laverock.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

cp "$ROOT/target/debug/laverock-desktop" "$APP/Contents/MacOS/Laverock"
cp "$ROOT/crates/laverock-desktop/icons/128x128.png" "$APP/Contents/Resources/icon.png"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Laverock</string>
  <key>CFBundleDisplayName</key>
  <string>Laverock</string>
  <key>CFBundleIdentifier</key>
  <string>sh.laverock.desktop</string>
  <key>CFBundleExecutable</key>
  <string>Laverock</string>
  <key>CFBundleIconFile</key>
  <string>icon.png</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>0.1.0</string>
  <key>CFBundleVersion</key>
  <string>0.1.0</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>Laverock 需要使用麦克风录制参考声音，用于在本机克隆音色。</string>
</dict>
</plist>
PLIST

# Ad-hoc signature gives macOS a stable identity for the microphone
# permission (TCC) prompt.
if command -v codesign >/dev/null 2>&1; then
  codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || true
fi

echo "Launching $APP"
open "$APP"
