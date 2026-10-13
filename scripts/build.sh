#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked
bundle="$PWD/dist/RingSwitch.app"
mkdir -p "$bundle/Contents/MacOS"
cp target/release/ringswitch "$bundle/Contents/MacOS/RingSwitch"
cat > "$bundle/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>RingSwitch</string>
<key>CFBundleIdentifier</key><string>local.zy.ringswitch</string>
<key>CFBundleName</key><string>RingSwitch</string>
<key>CFBundleDisplayName</key><string>RingSwitch</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.3.0</string>
<key>CFBundleVersion</key><string>3</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>LSUIElement</key><true/>
<key>NSPrincipalClass</key><string>NSApplication</string>
<key>NSInputMonitoringUsageDescription</key><string>监听 Option + ` 和鼠标侧键，以切换和排列窗口。</string>
</dict></plist>
PLIST
codesign --force --sign - --identifier local.zy.ringswitch "$bundle"
echo "Built: $bundle"
