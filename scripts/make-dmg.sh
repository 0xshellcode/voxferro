#!/bin/sh
# Builds target/dist/Voxferro-<version>-macos-<arch>.dmg from
# target/Voxferro.app (run scripts/bundle-macos.sh first); <arch> is the
# binary's, or "universal" when it holds both.
#
# With create-dmg (brew install create-dmg) the disk image opens on a window
# laid out for installing: the background from packaging/macos (drawn by
# dmg-background.swift, 1x and 2x combined into one TIFF), the app on the
# left, an arrow, and a link to Applications on the right. Without it, a
# plain image holding the app and the Applications link.
set -eu
cd "$(dirname "$0")/.."

app=target/Voxferro.app
[ -d "$app" ] || { echo "no $app: run scripts/bundle-macos.sh first" >&2; exit 1; }
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(lipo -archs "$app/Contents/MacOS/voxferro")
case "$arch" in *" "*) arch=universal ;; esac
mkdir -p target/dist
dmg="target/dist/Voxferro-$version-macos-$arch.dmg"
rm -f "$dmg"

stage=target/dmg-stage
rm -rf "$stage"
mkdir -p "$stage"
cp -R "$app" "$stage/"

if command -v create-dmg >/dev/null 2>&1; then
    tiffutil -cathidpicheck packaging/macos/background.png packaging/macos/background@2x.png \
        -out target/dmg-background.tiff
    # Positions are icon centres from the window's top left, matching the
    # arrow drawn on the background (y 190).
    create-dmg \
        --volname "Voxferro" \
        --volicon "$app/Contents/Resources/AppIcon.icns" \
        --background target/dmg-background.tiff \
        --window-pos 200 120 \
        --window-size 640 428 \
        --icon-size 112 \
        --text-size 13 \
        --icon "Voxferro.app" 170 190 \
        --hide-extension "Voxferro.app" \
        --app-drop-link 470 190 \
        --no-internet-enable \
        "$dmg" "$stage"
else
    ln -s /Applications "$stage/Applications"
    hdiutil create -volname Voxferro -srcfolder "$stage" -ov -format UDZO "$dmg" >/dev/null
fi
rm -rf "$stage"
echo "built $dmg"
