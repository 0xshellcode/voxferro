# Assets

## `vox/` — test corpus

Sample models shipped with the reference editor a recent version (macOS) and a recent version (win64),
copied here as a parser test corpus. `vox/anim/` holds the animated models,
which only the Windows build ships.

**These files are not ours.** They are authored by its author and distributed
with the reference editor, so they are deliberately listed in `.gitignore` and are not
committed: the repository must not redistribute someone else's assets.

To populate this directory on a fresh clone, copy the `vox/` folder out of a
local the reference editor install. Tests that need the corpus skip themselves when it
is absent, so a clean checkout still builds and tests green.

## `ibl/` — environment maps

The Radiance `.hdr` files the reference editor ships in its own `ibl/` folder, credited
in its readme to the HDRI-Hub free samples. Like `vox/`, they are third-party
and stay out of version control; copy them from a the reference editor install.

## `scripts/` — voxel scripts

Sample scripts for the `xs` command, written for Voxferro in WGSL and
licensed with it. the reference editor's own GLSL samples (its `shader/` folder) are
not copied here: the editor finds them in a local install, the same way it
finds environment maps, and runs them unchanged.

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
