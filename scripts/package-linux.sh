#!/bin/sh
# Builds Linux packages in target/dist:
#
# - voxferro-<version>-linux-<arch>.tar.gz, a prefix tree (bin/, share/):
#       tar -xzf voxferro-*.tar.gz -C ~/.local
#   installs the binary, the .desktop entry, the icons and the .vox MIME
#   type for the current user (then: update-desktop-database
#   ~/.local/share/applications; update-mime-database ~/.local/share/mime).
# - Voxferro-<version>-<arch>.AppImage, when appimagetool is on the PATH.
#
# The sample models and environment maps (assets/vox, assets/ibl), when
# present, are third-party and not in git; they go in only with
# --with-sample-assets, for a package you keep for yourself: do not
# redistribute them.
set -eu
cd "$(dirname "$0")/.."

with_samples=0
[ "${1:-}" = "--with-sample-assets" ] && with_samples=1

cargo build --release -p voxferro
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
dist=target/dist
root=$dist/voxferro-linux
rm -rf "$root"
mkdir -p "$root/bin" "$root/share/applications" "$root/share/mime/packages" \
         "$root/share/voxferro/assets"

cp target/release/voxferro "$root/bin/"
cp packaging/linux/voxferro.desktop "$root/share/applications/"
cp packaging/linux/voxferro-mime.xml "$root/share/mime/packages/voxferro.xml"
target/release/voxferro --make-icon-set assets/icon/icon.png "$root/share/icons/hicolor"
cp -R assets/showcase assets/scripts assets/materials_demo.vox "$root/share/voxferro/assets/"
if [ "$with_samples" = 1 ]; then
    for dir in vox ibl; do
        [ -d "assets/$dir" ] && cp -R "assets/$dir" "$root/share/voxferro/assets/"
    done
fi
cp LICENSE-MIT LICENSE-APACHE README.md "$root/share/voxferro/"

tarball=$dist/voxferro-$version-linux-$arch.tar.gz
tar -czf "$tarball" -C "$root" bin share
echo "built $tarball"

if command -v appimagetool >/dev/null 2>&1; then
    appdir=$dist/Voxferro.AppDir
    rm -rf "$appdir"
    mkdir -p "$appdir/usr"
    cp -R "$root/bin" "$root/share" "$appdir/usr/"
    cp packaging/linux/voxferro.desktop "$appdir/"
    cp "$root/share/icons/hicolor/256x256/apps/voxferro.png" "$appdir/voxferro.png"
    ln -s usr/bin/voxferro "$appdir/AppRun"
    ARCH=$arch appimagetool "$appdir" "$dist/Voxferro-$version-$arch.AppImage"
    echo "built $dist/Voxferro-$version-$arch.AppImage"
else
    echo "appimagetool not found: skipped the AppImage"
fi
