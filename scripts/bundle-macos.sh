#!/bin/sh
# Builds target/Voxferro.app: the release binary, an Info.plist, and
# the icon as an .icns made with sips and iconutil from the full-bleed
# assets/icon/icon-square.png (the system gives it its rounded shape).
set -eu
cd "$(dirname "$0")/.."
cargo build -q --release -p voxferro

app=target/Voxferro.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/voxferro "$app/Contents/MacOS/voxferro"
# Sample models and environment maps travel inside the bundle, so the app
# never reads the source tree (which macOS may guard, as in Documents).
mkdir -p "$app/Contents/Resources/assets"
cp -R assets/vox assets/ibl "$app/Contents/Resources/assets/"

iconset=target/AppIcon.iconset
rm -rf "$iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z $size $size assets/icon/icon-square.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z $double $double assets/icon/icon-square.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
rm -rf "$iconset"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Voxferro</string>
    <key>CFBundleDisplayName</key><string>Voxferro</string>
    <key>CFBundleIdentifier</key><string>io.github.0xshellcode.voxferro</string>
    <key>CFBundleExecutable</key><string>voxferro</string>
    <key>CFBundleIconFile</key><string>AppIcon</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
# Ad-hoc signature, so the bundle's resources are sealed and Gatekeeper
# treats it as a locally built app.
codesign --force --sign - "$app" >/dev/null 2>&1 || true
echo "built $app"
