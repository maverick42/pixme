#!/usr/bin/env bash
# Packages the release build as an ad-hoc-signed PixMe.app inside a
# DMG at target/release/PixMe-<version>.dmg.
#
# Ad-hoc signing is required for capture testing (Screen Recording
# permission only sticks to signed bundles). The DMG is a dev
# artefact — no notarization, Gatekeeper's "unidentified developer"
# prompt is expected on first open.
set -euo pipefail
cd "$(dirname "$0")/.."

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
app="target/bundle/PixMe.app"
dmg="target/release/pixme-${version}-macos-${arch}.dmg"

cargo build --release

rm -rf target/bundle
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/pixme "$app/Contents/MacOS/pixme"
cp assets/app/PixMe.icns "$app/Contents/Resources/PixMe.icns"
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>pixme</string>
    <key>CFBundleIdentifier</key>
    <string>com.maverick42.pixme</string>
    <key>CFBundleName</key>
    <string>PixMe</string>
    <key>CFBundleDisplayName</key>
    <string>PixMe</string>
    <key>CFBundleIconFile</key>
    <string>PixMe.icns</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSScreenCaptureUsageDescription</key>
    <string>PixMe needs Screen Recording to capture screenshots.</string>
</dict>
</plist>
EOF

codesign --force --deep -s - "$app"

rm -rf target/dmg "$dmg"
mkdir -p target/dmg
cp -R "$app" target/dmg/
ln -s /Applications target/dmg/Applications
hdiutil create -volname PixMe -srcfolder target/dmg -ov -format UDZO "$dmg" >/dev/null
rm -rf target/dmg

echo "Packaged: $dmg"
