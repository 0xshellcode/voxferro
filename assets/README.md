# Assets

## `vox/` — test corpus

Sample `.vox` models used as a parser test corpus; `vox/anim/` holds the
animated ones.

**These files are not ours.** They are third-party, so they are deliberately
listed in `.gitignore` and are not committed: the repository must not
redistribute someone else's assets.

On a fresh clone this directory is empty; put `.vox` models in it to run the
tests that use them. Tests that need the corpus skip themselves when it is
absent, so a clean checkout still builds and tests green.

## `ibl/` — environment maps

Radiance `.hdr` environment maps, the only place the editor looks for them.
Like `vox/`, they are third-party and stay out of version control.

## `scripts/` — voxel scripts

Sample scripts for the `xs` command, written for Voxferro in WGSL and
licensed with it. GLSL voxel scripts run unchanged too: the editor finds
them here and in the user's settings folder.

## `showcase/` — gallery scenes

The README's gallery: each scene as a list of console commands (`.cmds`,
run with `--script`), the `.vox` it builds, and the images rendered from
it. Made with Voxferro and licensed with it; `shapes.cmds` is also the CI
smoke test's scene.

## `icon/` — the app icon

`icon.cmds` builds the icon's model. `scripts/make-icon.sh` (or
`make-icon.ps1`) path traces it into `icon.png` (with the macOS icon shape,
transparent around it) and `icon-square.png` (filling the canvas, for the
`.icns`), and makes the Windows `icon.ico` from `icon.png`.

## `materials_demo.vox`

A small scene with metal, glass and emissive materials, written by
`cargo run -p voxel-core --example materials_demo`. It is bundled with the
packages.
