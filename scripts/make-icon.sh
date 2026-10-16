#!/bin/sh
# Renders the app icon (assets/icon/icon.png) from its model, which is a
# list of editor commands (assets/icon/icon.cmds), with the path tracer, and
# the .ico the Windows executable embeds.
set -eu
cd "$(dirname "$0")/.."
cargo build -q -p voxferro
./target/debug/voxferro --script assets/icon/icon.cmds \
    --run "camera frame" --run "camera zoom 1.05" --camera -55 28 \
    --render 512 --icon assets/icon/icon.png
./target/debug/voxferro --make-ico assets/icon/icon.png assets/icon/icon.ico
