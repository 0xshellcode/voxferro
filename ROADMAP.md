# Roadmap

A phased plan. Each phase ends in something runnable and verifiable: no
refactors without a visible payoff.

Phases 0–7 built a basic viewer and editor; phases 8–13 the path tracer.
From Phase 14 on the focus is the **editor**: correct multi-model scenes
first, then the tools that make it usable.

| Phase | Deliverable | Status |
|---|---|---|
| 0 | Workspace, verified dependencies, backend reported | ✅ |
| 1 | `.vox` parser, validated against a corpus of files and against `dot_vox` | ✅ |
| 2 | Window, wgpu pipeline and orbit camera | ✅ |
| 3 | Viewer: a `.vox` model on screen in colour | ✅ |
| 4 | Multi-model scenes with transforms | ✅ |
| 5 | Mouse editing + undo/redo | ✅ |
| 6 | Palette, layers and command console | ✅ |
| 7 | Voxel operations as compute kernels | ✅ |
| 8 | `.vox` writing with verified round-trip | ✅ |
| 9 | Progressive path tracer with lighting from `rOBJ` | ✅ |
| 10 | Materials: metal, glass, emissive | ✅ |
| 11 | Ground plane and default lighting | ✅ |
| 12 | HDR map lighting (IBL) | ✅ |
| 13 | Atmospheric sky (Rayleigh and Mie scattering) | ✅ |
| 14 | Correct multi-model scenes in the tracer and in editing | ✅ |
| 15 | Files: open, new, save as | ✅ |
| 16 | Brushes: box, line, face, fill | ✅ |
| 17 | Selection, copy/paste and model transforms | ✅ |
| 18 | Palette and material editing from the UI | ✅ |
| 19 | OBJ export with greedy meshing | ✅ |
| 20 | Scene graph writing: move, rotate, duplicate, add and remove | ✅ |
| 21 | Camera: frame, axis views, orthographic, pan | ✅ |
| 22 | Render settings from the UI, saved in the file | ✅ |
| 23 | Scene groups and layers | ✅ |
| 24 | Lens (depth of field), vignette and bloom | ✅ |
| 25 | Rotate groups and drag in the scene tree | ✅ |
| 26 | Rotations in x, y, z and selecting groups in the viewport | ✅ |
| 27 | Move tool: drag and rotate with the mouse | ✅ |
| 28 | Animation: playback, keys and shape frames | ✅ |
| 29 | Range, timeline and animation export | ✅ |
| 30 | Move keys by dragging them on the timeline | ✅ |
| 31 | Media: fog and clouds | ✅ |
| 32 | Exposure and shadow calibration | ✅ |
| 33 | Performance with large models | ✅ |
| 34 | App icon and `.app` bundle | ✅ |
| 35 | Control via MCP | ✅ |
| 36 | Drawing on an empty model | ✅ |
| 37 | More efficient MCP: batches, palette, covered window | ✅ |
| 38 | Brush symmetry and eyedropper | ✅ |
| 39 | Saved cameras and render capture to PNG | ✅ |
| 40 | Open `.vox` from Finder and recent files | ✅ |
| 41 | Windows version, checked on a Windows machine with real GPUs | ✅ |
| 42 | Linux version | ⏳ built and tested in CI; needs a Linux desktop |
| 43 | Voxel scripts (`xs`), in WGSL and GLSL | ✅ |
| 44 | The Edit panel's transforms | ✅ |
| 45 | Brushes: shader, pattern, magic wand, axis lock | ✅ |
| 46 | Project panel and palette files | ✅ |
| 47 | Export formats and file operations | ✅ |
| 48 | Display switches and camera modes | ✅ |
| 49 | Brush options and transforms | ✅ |
| 50 | Marching cubes, sprites, mesh import, mouse transforms | ✅ |

Since Phase 50 the work has gone on feature by feature: the panel layout,
releases, performance at 256³, transform wrap and the camera ruler, maze,
Perlin and fractal generators, autosave, fuzzing the file readers, the
keyboard shortcuts, the final render at its own size, objects (World mode),
palette, render, animation and camera tools, Blend lamps, render lines,
marching cubes as a render shape, global time, the render's sample and lens
options, the outline's menus, the editor's view options, palette notes,
dropped files, selective exports, packing models into animations, and a
pass on Windows with real GPUs (each sample traced in bands so a slow GPU
is never reset mid-render; Vulkan preferred over DirectX 12).

**301 tests green.**

## Not done yet

- **Linux desktop:** X11 and Wayland with a hardware driver, the portal
  file dialogs, and opening a `.vox` from the file manager.
- **Signing:** the app signed and notarized for macOS (requires an Apple
  Developer account), and the Windows executable signed, so SmartScreen
  does not stop it.
- **Voxel shapes under DirectX 12** are several times slower than under
  Vulkan, which Windows now prefers: a build per shape, or a pipeline
  cache, would help machines without a Vulkan driver.
- **Glass drawn as marching cubes** shows dark seams: the walk inside
  glass goes cell by cell while the entry is on the smooth surface.
- Lines (grid and edges) in the render are drawn on cubes only.
