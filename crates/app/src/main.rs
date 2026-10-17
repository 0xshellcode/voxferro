// A release build on Windows is a GUI app: no console window opens with
// it. `attach_parent_console` below keeps the output of headless runs
// started from a terminal.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! Entry point for the voxel editor.
//!
//! eframe owns the window and the event loop. The scene is rendered with wgpu
//! into an offscreen texture that egui then displays, which is what gives us a
//! depth buffer (see `renderer`). On Apple Silicon wgpu emits Metal directly,
//! so nothing goes through the deprecated OpenGL stack.

mod anim_ui;
mod camera;
mod camera_tools;
mod compute;
mod components;
mod console;
mod edit;
mod exchange;
mod files;
mod icon;
mod image_out;
mod mcp;
mod mcp_ui;
mod model_edit;
#[cfg(target_os = "macos")]
mod macos_open;
mod recent;
mod palette_files;
mod palette_ui;
mod project;
mod move_tool;
mod outline;
mod pattern;
mod pathtrace;
mod render_ui;
mod renderer;
mod scene;
mod scene_ui;
mod select;
mod tools;
mod transform;
mod view_cube;
mod view_options;
mod trace_cache;
#[cfg(windows)]
mod windows_graphics;
mod xs;
mod xs_ui;

use camera::OrbitCamera;
use eframe::{egui, egui_wgpu};
use glam::Vec3;
use renderer::SceneRenderer;
use pathtrace::{Environment, PathTracer};
use voxel_core::{History, Mesh};

/// On Windows, a GUI-subsystem program has no console. When it was started
/// from one (`voxferro --run ... --screenshot out.bmp` in a terminal),
/// attach to it so `eprintln!` reaches the user; from Explorer there is no
/// parent console and this does nothing.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: a plain Win32 call with a constant argument.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

fn main() -> eframe::Result {
    #[cfg(windows)]
    attach_parent_console();

    // Tools that need no window.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [flag, png, ico] = args.as_slice()
        && flag == "--make-ico"
    {
        match icon::make_ico(png, ico) {
            Ok(()) => eprintln!("[voxferro] wrote {ico}"),
            Err(e) => {
                eprintln!("[voxferro] {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    if let [flag, png, dir] = args.as_slice()
        && flag == "--make-icon-set"
    {
        match icon::make_icon_set(png, dir) {
            Ok(()) => eprintln!("[voxferro] wrote icons in {dir}"),
            Err(e) => {
                eprintln!("[voxferro] {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // Before the event loop, so a file that launches the app is caught.
    #[cfg(target_os = "macos")]
    macos_open::install();

    // Mutated on Windows only, where the graphics backend is chosen.
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Voxferro")
            // Window and Dock icon when run as a bare binary; the .app
            // bundle carries its own .icns (scripts/bundle-macos.sh).
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon/icon.png"))
                    .expect("the bundled icon is a valid PNG"),
            ),
        ..Default::default()
    };

    #[cfg(windows)]
    if let Some(message) = windows_graphics::configure(&mut options) {
        eprintln!("[voxferro] {message}");
    }

    eframe::run_native(
        "Voxferro",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// The `assets` folder, first found of:
///
/// - inside the macOS `.app` bundle (`Contents/Resources/assets`);
/// - next to the executable (`assets/` beside `voxferro.exe` in the
///   Windows zip);
/// - the Linux layout, `<prefix>/share/voxferro/assets` for
///   `<prefix>/bin/voxferro` (the tarball and the AppImage);
/// - the source tree's, resolved at compile time so `cargo run` works from
///   any directory.
///
/// A packaged app never reaches back into the source tree, which may sit in
/// a folder macOS guards (Documents) and would ask for permission.
fn assets_dir() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|d| d.to_path_buf()));
    let packaged = exe_dir.and_then(|dir| {
        let bundle = dir.parent().map(|contents| contents.join("Resources/assets"));
        let share = dir.parent().map(|prefix| prefix.join("share/voxferro/assets"));
        [bundle, Some(dir.join("assets")), share].into_iter().flatten().find(|p| p.is_dir())
    });
    packaged.unwrap_or_else(|| std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets")))
}

/// Where the bundled sample models live.
fn sample_dir() -> std::path::PathBuf {
    assets_dir().join("vox")
}

/// Local the reference editor installs, whose folders (`ibl/`, `shader/`) the
/// editor falls back to:
///
/// - `reference_editor_DIR`, if set: one install's folder, on any system;
/// - every folder in `~/Applications/the reference editor` -- the home being `HOME`,
///   or `USERPROFILE` on Windows -- where several versions can sit side by
///   side.
fn reference_editor_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("reference_editor_DIR") {
        dirs.push(std::path::PathBuf::from(dir));
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let apps = std::path::Path::new(&home).join("Applications").join("the reference editor");
        if let Ok(entries) = std::fs::read_dir(apps) {
            let mut installs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            // Newest version first, so its copy of a script wins.
            installs.sort_by(|a, b| b.cmp(a));
            dirs.extend(installs);
        }
    }
    dirs
}

/// Where environment maps are looked for, in order. A file names its map
/// relative to the editor's own `ibl/` folder, so after the bundled copies
/// the search falls back to a local the reference editor install.
fn env_map_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![assets_dir().join("ibl")];
    dirs.extend(reference_editor_dirs().into_iter().map(|d| d.join("ibl")));
    dirs
}

/// Where voxel scripts are looked for (see `xs::dirs`).
fn script_dirs() -> Vec<std::path::PathBuf> {
    xs::dirs(&assets_dir(), &reference_editor_dirs())
}

fn find_env_map(name: &str) -> Option<std::path::PathBuf> {
    // A bare file name only: the path comes from a file we did not write.
    let name = std::path::Path::new(name).file_name()?;
    env_map_dirs()
        .into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

struct App {
    backend: String,
    camera: OrbitCamera,
    scene: Option<scene::LoadedScene>,
    /// Owned here rather than in egui's `callback_resources`: rendering needs
    /// `&mut` on egui's own renderer at the same time (to register the target
    /// texture), and both cannot be borrowed out of the same map at once.
    scene_renderer: SceneRenderer,
    voxel_compute: compute::VoxelCompute,
    /// Voxel scripts: the runner and the Scripts panel's state.
    xs: xs_ui::XsState,
    path_tracer: PathTracer,
    /// Render mode, mirroring the editor's Model/Render tabs.
    rendering: bool,
    environment: Environment,
    /// Depth of field, for the session: the file format has no key for it.
    dof: bool,
    /// `--timings`: report how long each rebuild takes.
    timings: bool,
    /// Meshes per model chunk, so an edit re-meshes only what it touched.
    mesh_cache: voxel_core::mesh_cache::MeshCache,
    /// The tracer's grid as last baked, to update it in place.
    trace_cache: Option<trace_cache::TraceCache>,
    /// Animation playback.
    playing: bool,
    fps: f32,
    /// Seconds of playback not yet turned into frames.
    play_clock: f64,
    /// A key being dragged on the timeline.
    key_drag: Option<anim_ui::KeyDrag>,
    /// Frames being rendered to files, and the samples a frame gets.
    export: Option<anim_ui::ExportJob>,
    export_samples: u32,
    /// The render objects `environment` was decoded from.
    environment_source: Vec<vox_format::Dict>,
    /// Environment map names found on disk, listed on first use.
    env_maps: Option<Vec<String>>,
    /// Samples still to accumulate before a headless screenshot.
    pending_samples: u32,
    status: String,
    /// The placed instance edits, kernels and console commands act on: the
    /// one last clicked.
    selected: usize,
    /// Instances picked together in the outline (Cmd+click), for grouping.
    picked: std::collections::BTreeSet<usize>,
    /// A group selected in the outline, by its transform node.
    selected_group: Option<i32>,
    /// World cell of the traced grid's corner. The tracer walks one grid
    /// holding every visible instance; this maps the camera into it.
    trace_origin: glam::IVec3,
    light_dir: Vec3,
    history: History,
    /// Palette index the brush paints with.
    brush_color: u8,
    brush_mode: voxel_core::BrushMode,
    /// Brush symmetry: strokes repeat reflected through the model's centre
    /// on these axes.
    mirror: [bool; 3],
    /// Axis mode: strokes run through the whole model on these axes.
    axis_lock: [bool; 3],
    /// The Brush panel's options: voxel size and tip, square and circle,
    /// line projection, the face brush's region and colour.
    brush_options: voxel_core::brush::BrushOptions,
    /// What a Fill click takes in, when erasing, painting or selecting.
    region: voxel_core::brush::Region,
    /// The pattern brush's model, and whether it keeps its own colours.
    pattern: Option<voxel_core::Volume>,
    pattern_own_colours: bool,
    /// Whether a pattern opened from another file takes this palette's
    /// nearest colours.
    pattern_match: bool,
    /// Colours copied with `palette copy`.
    palette_clip: Vec<vox_format::Color>,
    /// The GPU, for the shader brush, which runs from mouse handling.
    gpu: egui_wgpu::RenderState,
    /// The next click takes its colour from a voxel instead of editing.
    picking: bool,
    /// The saved-camera slot the camera panel acts on.
    camera_slot: usize,
    /// Recently opened and saved files, and whether to add to them: not
    /// in scripted runs (`--run`, `--screenshot`, ...), which would fill
    /// the list with test files.
    recent: Vec<String>,
    remember_files: bool,
    brush_shape: voxel_core::BrushShape,
    /// The brush stroke under way, while the button is held.
    stroke: Option<tools::Stroke>,
    /// A press on empty space turns the drag into an orbit.
    orbiting: bool,
    /// The Select tool is active instead of a brush.
    selecting: bool,
    /// The Move tool is active instead of a brush.
    moving: bool,
    move_drag: Option<move_tool::MoveDrag>,
    selection: Option<voxel_core::Selection>,
    select_drag: Option<select::SelectDrag>,
    clipboard: voxel_core::Clipboard,
    /// The Resize fields, and which model and size they were filled from.
    resize_to: [u32; 3],
    /// The viewport's display switches.
    view: view_options::ViewOptions,
    /// The instance the mesh was built around, while the background is
    /// hidden: selecting another must remesh.
    meshed_for: Option<usize>,
    /// The Transform tool's mouse handle, when armed, and a transform in
    /// progress.
    transform_tool: Option<transform::Handle>,
    transform_drag: Option<transform::TransformDrag>,
    /// A `snap window` waiting for the window's screenshot.
    window_snap: Option<String>,
    window_snap_sent: bool,
    /// The view cube in the viewport's corner.
    view_cube_shown: bool,
    /// A `snap` waiting for the viewport's first frame.
    pending_snap: Option<String>,
    /// The Edit panel's text fields.
    edit_fields: model_edit::Fields,
    /// The Project panel's folder and its pending confirmation.
    project: project::ProjectState,
    resize_for: Option<(usize, [u32; 3])>,
    /// Set when the scene changed and the GPU mesh is stale.
    mesh_dirty: bool,
    console_input: String,
    console_log: Vec<String>,
    /// When set, the app writes one frame of the whole window here and exits.
    /// Used to verify the UI without a human looking at the window, and to
    /// produce images for the README.
    screenshot: Option<String>,
    /// Write the screenshot as the app icon (`--icon`).
    write_icon: bool,
    /// The viewport's size in pixels at its last draw, for drawing while
    /// the window is hidden.
    last_view_size: [u32; 2],
    /// The guide lines last uploaded (see `model_guides`).
    guide_lines: Vec<renderer::LineVertex>,
    /// The MCP server, while switched on, and the port it takes.
    mcp: Option<mcp::Server>,
    mcp_port: u16,
    /// An agent's write over an existing file, waiting for the user.
    mcp_write: Option<mcp_ui::PendingWrite>,
    /// An agent's capture, waiting for its frame.
    mcp_capture: Option<mcp_ui::PendingCapture>,
    /// Strokes given on the command line (`--click`, `--drag`), as press and
    /// release points in fractions of the viewport. They run on the first
    /// frame, once the viewport's size is known, through the same calls a
    /// mouse stroke makes.
    scripted_strokes: Vec<(egui::Vec2, egui::Vec2)>,
    /// `history.revision()` at the last load or save.
    saved_revision: u64,
    /// The unsaved-changes prompt, while it is open.
    confirm: Option<files::Confirm>,
    /// Set once the user agreed to quit, so the close is not intercepted.
    allow_close: bool,
    /// Last title sent to the window, to send it only when it changes.
    window_title: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let state = cc
            .wgpu_render_state
            .as_ref()
            .expect("eframe must run on wgpu; check that its wgpu feature is enabled");

        let info = state.adapter.get_info();
        let backend = format!("{:?} on {}", info.backend, info.name);
        eprintln!("[voxferro] graphics backend: {backend}");

        let mut app = Self {
            backend,
            camera: OrbitCamera::default(),
            scene: None,
            scene_renderer: SceneRenderer::new(&state.device, state.target_format),
            voxel_compute: compute::VoxelCompute::new(&state.device),
            xs: xs_ui::XsState::new(&state.device),
            path_tracer: PathTracer::new(&state.device),
            rendering: false,
            environment: Environment::default(),
            env_maps: None,
            environment_source: Vec::new(),
            dof: false,
            timings: std::env::args().any(|a| a == "--timings"),
            mesh_cache: Default::default(),
            trace_cache: None,
            playing: false,
            fps: 10.0,
            play_clock: 0.0,
            export: None,
            key_drag: None,
            export_samples: 64,
            pending_samples: 0,
            status: String::new(),
            selected: 0,
            picked: Default::default(),
            selected_group: None,
            trace_origin: glam::IVec3::ZERO,
            light_dir: Vec3::new(0.3, 0.5, 0.8).normalize(),
            screenshot: None,
            write_icon: false,
            guide_lines: Vec::new(),
            last_view_size: [1024, 768],
            mcp: None,
            mcp_port: mcp::DEFAULT_PORT,
            mcp_write: None,
            mcp_capture: None,
            scripted_strokes: Vec::new(),
            saved_revision: 0,
            confirm: None,
            allow_close: false,
            window_title: String::new(),
            history: History::default(),
            brush_color: 79,
            brush_mode: voxel_core::BrushMode::Attach,
            mirror: [false; 3],
            axis_lock: [false; 3],
            brush_options: voxel_core::brush::BrushOptions::default(),
            region: voxel_core::brush::Region::default(),
            pattern: None,
            pattern_own_colours: true,
            pattern_match: true,
            palette_clip: Vec::new(),
            gpu: state.clone(),
            picking: false,
            camera_slot: 0,
            recent: recent::load(),
            remember_files: true,
            brush_shape: voxel_core::BrushShape::Voxel,
            stroke: None,
            orbiting: false,
            selecting: false,
            moving: false,
            move_drag: None,
            selection: None,
            select_drag: None,
            clipboard: voxel_core::Clipboard::default(),
            resize_to: [32; 3],
            view: view_options::ViewOptions::default(),
            meshed_for: None,
            pending_snap: None,
            view_cube_shown: true,
            window_snap: None,
            transform_tool: None,
            transform_drag: None,
            window_snap_sent: false,
            edit_fields: model_edit::Fields::default(),
            project: project::ProjectState::default(),
            resize_for: None,
            mesh_dirty: false,
            console_input: String::new(),
            console_log: vec![
                "type 'help', or 'gpu sphere' to run a compute kernel".to_owned(),
            ],
        };

        // Usage: voxferro [model.vox] [--run "<command>"]... [--render-mode]
        //           [--camera azimuth elevation] [--render N]
        //           [--screenshot out.bmp] [--icon out.png] [--mcp [port]]
        let args: Vec<String> = std::env::args().skip(1).collect();
        let screenshot = args
            .iter()
            .position(|a| a == "--screenshot")
            .and_then(|i| args.get(i + 1).cloned());

        // --render N switches to the path tracer and accumulates N samples
        // before the screenshot is taken.
        let render_samples = args
            .iter()
            .position(|a| a == "--render")
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<u32>().ok());
        let path = positional_args(&args)
            .into_iter()
            .find(|a| a.ends_with(".vox"))
            .unwrap_or_else(|| sample_dir().join("monu1.vox").to_string_lossy().into_owned());

        // Several --run flags may be given; they execute in order, which is
        // how a kernel and a save fit into one headless session.
        // --script <file> runs a file of commands, one per line, in the
        // same order; blank lines and lines starting with # are skipped.
        let mut startup_commands = Vec::new();
        for (i, a) in args.iter().enumerate() {
            let Some(value) = args.get(i + 1) else { continue };
            match a.as_str() {
                "--run" => startup_commands.push(value.clone()),
                "--script" => match std::fs::read_to_string(value) {
                    Ok(text) => startup_commands.extend(
                        text.lines()
                            .map(str::trim)
                            .filter(|l| !l.is_empty() && !l.starts_with('#'))
                            .map(str::to_owned),
                    ),
                    Err(e) => eprintln!("[voxferro] could not read {value}: {e}"),
                },
                _ => {}
            }
        }

        // --click <fx> <fy>: a click at that fraction of the viewport.
        // --drag <fx0> <fy0> <fx1> <fy1>: press, move, release.
        app.scripted_strokes = args
            .iter()
            .enumerate()
            .filter_map(|(i, a)| {
                let n = |k: usize| args.get(i + k)?.parse::<f32>().ok();
                match a.as_str() {
                    "--click" => {
                        let p = egui::vec2(n(1)?, n(2)?);
                        Some((p, p))
                    }
                    "--drag" => Some((egui::vec2(n(1)?, n(2)?), egui::vec2(n(3)?, n(4)?))),
                    _ => None,
                }
            })
            .collect();

        // --icon <out.png>: like --screenshot, written as the app icon.
        let icon = args.iter().position(|a| a == "--icon").and_then(|i| args.get(i + 1).cloned());
        app.write_icon = icon.is_some();
        app.screenshot = icon.or(screenshot);
        let scripted = ["--run", "--script", "--screenshot", "--icon", "--render", "--click", "--drag"]
            .iter()
            .any(|f| args.iter().any(|a| a == f))
            || std::env::var_os("VOXFERRO_NO_RECENT").is_some();
        app.remember_files = !scripted;
        app.load(state, &path);
        // No model to start from -- a fresh clone has no the reference editor
        // samples, or the file named is gone: start with an empty one, so
        // there is something to draw in. The load error stays in the log.
        if app.scene.is_none() {
            app.install(state, scene::LoadedScene::new_empty(files::NEW_MODEL_SIZE));
        }
        // A file named on the command line (or by the Explorer, or a
        // .desktop file) is one the user opened; the bundled sample is not.
        if positional_args(&args).iter().any(|a| a == &path) && app.scene.is_some() {
            app.remember(&path);
        }
        #[cfg(target_os = "macos")]
        macos_open::set_waker(&cc.egui_ctx);

        // Run before the first frame, so --screenshot captures the result.
        for command in &startup_commands {
            app.run_command(command, &cc.egui_ctx, state);
            if app.mesh_dirty {
                app.remesh(state);
            }
        }
        if !startup_commands.is_empty() {
            for line in &app.console_log {
                eprintln!("[voxferro] {line}");
            }
        }

        // --camera <azimuth> <elevation>, degrees: view direction for
        // reproducible renders. Azimuth -90 looks from -Y toward +Y, the
        // editor's default front view.
        let camera_angles = args.iter().position(|a| a == "--camera").and_then(|i| {
            let yaw = args.get(i + 1)?.parse::<f32>().ok()?;
            let pitch = args.get(i + 2)?.parse::<f32>().ok()?;
            Some((yaw, pitch))
        });
        if let Some((yaw, pitch)) = camera_angles {
            app.camera.yaw = yaw.to_radians();
            app.camera.pitch = pitch.to_radians();
        }

        // --render-mode opens straight into the path tracer, interactively.
        if args.iter().any(|a| a == "--render-mode") {
            app.rendering = true;
        }

        // --mcp [port]: let agents use the editor from the start.
        if let Some(i) = args.iter().position(|a| a == "--mcp") {
            if let Some(port) = args.get(i + 1).and_then(|p| p.parse::<u16>().ok()) {
                app.mcp_port = port;
            }
            app.start_mcp(&cc.egui_ctx);
        }

        if let Some(samples) = render_samples {
            app.rendering = true;
            app.pending_samples = samples;
        }
        app
    }

    fn load(&mut self, state: &egui_wgpu::RenderState, path: &str) {
        match scene::LoadedScene::load(path) {
            Ok(loaded) => self.install(state, loaded),
            Err(e) => {
                self.status = format!("failed to load {path}: {e}");
                self.console_log.push(self.status.clone());
            }
        }
    }

    /// Makes `loaded` the open document.
    fn install(&mut self, state: &egui_wgpu::RenderState, loaded: scene::LoadedScene) {
        self.status = format!(
            "{} models, {} placed, v{}",
            loaded.doc.scene.models.len(),
            loaded.doc.scene.instances.len(),
            loaded.version
        );
        self.selected = 0;
        self.selection = None;
        self.forget_picks();
        // Model indices in the caches belong to the old file.
        self.mesh_cache = Default::default();
        self.trace_cache = None;
        self.playing = false;
        // Undo steps name models by index: they mean nothing in another
        // file.
        self.history.clear();
        self.saved_revision = self.history.revision();
        self.scene = Some(loaded);
        self.upload_current(state);
        self.sync_environment(state);
    }

    /// Meshes the whole scene and hands it to the GPU.
    ///
    /// Every instance is baked into one world-space mesh. Instanced draws
    /// would scale better for a scene that repeats one model many times, but
    /// a single buffer keeps the draw path trivial while the geometry fits.
    fn upload_current(&mut self, state: &egui_wgpu::RenderState) {
        let mesh = self.build_mesh();
        // Disjoint field borrows: `scene` is read while `scene_renderer` is
        // mutated, which the borrow checker allows because they are separate
        // fields accessed directly rather than through a method on `self`.
        let Some(loaded) = &self.scene else { return };
        eprintln!(
            "[voxferro] {}: {} instances, {} voxels, {} triangles",
            loaded.name,
            loaded.doc.scene.instances.len(),
            loaded.doc.scene.total_voxels(),
            mesh.triangle_count()
        );

        if let Some((min, max)) = mesh.bounds().or_else(|| loaded.doc.scene.declared_bounds()) {
            self.camera.frame_world_bounds(min, max);
        }
        self.scene_renderer.set_mesh(&state.device, &mesh);
        self.scene_renderer.set_wire(&state.device, self.view.wire.then_some(&mesh));
        self.meshed_for = (!self.view.background).then_some(self.selected);
        self.refresh_colors(state);
        self.upload_trace_volume(state);
    }

    /// Rebuilds the GPU mesh after an edit, leaving the camera alone.
    fn remesh(&mut self, state: &egui_wgpu::RenderState) {
        if self.scene.is_none() {
            return;
        }
        let started = std::time::Instant::now();
        let mesh = self.build_mesh();
        let meshed = started.elapsed();
        self.scene_renderer.set_mesh(&state.device, &mesh);
        self.scene_renderer.set_wire(&state.device, self.view.wire.then_some(&mesh));
        self.meshed_for = (!self.view.background).then_some(self.selected);
        let uploaded = started.elapsed();
        self.sync_trace_volume(state);
        if self.timings {
            eprintln!(
                "[voxferro] remesh: mesh {:.1?} ({} chunks), upload {:.1?}, trace grid {:.1?}",
                meshed,
                self.mesh_cache.last_rebuilt,
                uploaded - meshed,
                started.elapsed() - uploaded
            );
        }
        self.mesh_dirty = false;
    }

    /// The viewport mesh: visible layers only, selected voxels flagged.
    fn build_mesh(&mut self) -> Mesh {
        let Some(loaded) = &self.scene else { return Mesh::default() };
        let visible = self.visibility();
        let selection = self.selection.as_ref();
        // With the background hidden, the selected model alone.
        let only = (!self.view.background)
            .then(|| loaded.doc.scene.instances.get(self.selected))
            .flatten()
            .map(|i| i as *const voxel_core::Instance);
        self.mesh_cache.build(
            &loaded.doc.scene,
            |i| visible.shows(i) && only.is_none_or(|o| std::ptr::eq(o, i)),
            |model| selection.filter(|s| s.model == model).map(|s| s.cells()),
        )
    }

    /// Hands the palette and materials to both renderers. Cheap: two small
    /// buffer writes, so any palette or material edit just calls this.
    fn refresh_colors(&mut self, state: &egui_wgpu::RenderState) {
        let Some(loaded) = &self.scene else { return };
        let doc = &loaded.doc;
        self.scene_renderer.set_palette(&state.queue, &doc.palette);
        self.path_tracer.set_palette(&state.queue, &doc.palette);
        self.path_tracer.set_materials(&state.queue, &doc.palette, &doc.materials);
    }

    /// Bakes every visible instance into one world-aligned grid for the
    /// tracer, which walks voxels rather than the mesh.
    ///
    /// Emitters come from the same grid, so a lamp on any model lights the
    /// scene, and so does a material change that turns a colour emissive.
    fn upload_trace_volume(&mut self, state: &egui_wgpu::RenderState) {
        let Some(loaded) = &self.scene else { return };
        let visible = self.visibility();
        // A 3D texture is capped at 2048 per side on Metal; the cell budget
        // keeps a scene of far-apart models from allocating gigabytes.
        let max_side = state.device.limits().max_texture_dimension_3d;
        let baked = loaded
            .doc
            .scene
            .bake(|i| visible.shows(i), 256 * 1024 * 1024)
            .or_else(|e| match e {
                // Nothing to trace still has a sky and a ground to render:
                // an empty one-cell grid at the origin. Without it the
                // render stayed blank, and a headless `--render` of an
                // empty model waited forever for its first sample.
                voxel_core::BakeError::Empty => Ok(voxel_core::Baked {
                    volume: voxel_core::Volume::new([1, 1, 1]),
                    origin: glam::IVec3::ZERO,
                }),
                other => Err(other),
            })
            .map_err(|e| e.to_string())
            .and_then(|b| {
                if b.volume.size().iter().any(|&s| s > max_side) {
                    let [x, y, z] = b.volume.size();
                    Err(format!("scene spans {x}x{y}x{z} voxels, over the GPU limit of {max_side}"))
                } else {
                    Ok(b)
                }
            });
        match baked {
            Ok(baked) => {
                self.path_tracer.set_volume(&state.device, &state.queue, &baked.volume);
                self.path_tracer
                    .set_emitters(&state.device, &baked.volume, &loaded.doc.materials);
                self.trace_origin = baked.origin;
                self.remember_bake(baked);
            }
            Err(e) => {
                self.path_tracer.clear_volume();
                self.trace_cache = None;
                eprintln!("[voxferro] path tracer: {e}");
            }
        }
    }

    /// After undo or redo: the step may have touched voxels, colours or
    /// materials, so everything is refreshed.
    fn refresh_document(&mut self, state: &egui_wgpu::RenderState) {
        // The step may have been a render setting.
        if self.scene.as_ref().is_some_and(|s| s.doc.render != self.environment_source) {
            self.sync_environment(state);
        }
        // Undoing an added instance can leave the selection past the end.
        let count = self.scene.as_ref().map_or(0, |s| s.doc.scene.instances.len());
        self.selected = self.selected.min(count.saturating_sub(1));
        self.revalidate_selection();
        self.refresh_colors(state);
        self.remesh(state);
    }

    /// The selected instance's model index, if there is one.
    fn selected_model(&self) -> Option<usize> {
        let loaded = self.scene.as_ref()?;
        let instance = loaded.doc.scene.instances.get(self.selected)?;
        Some(instance.model_index)
    }

    fn undo(&mut self, state: &egui_wgpu::RenderState) {
        let Some(loaded) = self.scene.as_mut() else { return };
        if self.history.undo(&mut loaded.doc) {
            // Undoing a grouping renumbers nodes: a selected group id would
            // now name some other node.
            self.forget_picks();
            self.refresh_document(state);
        }
    }

    fn redo(&mut self, state: &egui_wgpu::RenderState) {
        let Some(loaded) = self.scene.as_mut() else { return };
        if self.history.redo(&mut loaded.doc) {
            self.forget_picks();
            self.refresh_document(state);
        }
    }

    /// Shift from world space into the traced grid's voxel space.
    fn volume_offset(&self) -> Vec3 {
        -self.trace_origin.as_vec3()
    }

    fn controls(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        ui.heading("Voxferro");
        ui.label(&self.backend);
        ui.separator();

        let ctx = ui.ctx().clone();
        ui.horizontal_wrapped(|ui| {
            if ui.button("New").on_hover_text("Cmd+N").clicked() {
                self.request(files::Discarding::New(files::NEW_MODEL_SIZE), &ctx, state);
            }
            if ui.button("Open…").on_hover_text("Cmd+O").clicked() {
                self.request(files::Discarding::Open(None), &ctx, state);
            }
            let has_doc = self.scene.is_some();
            if ui.add_enabled(has_doc, egui::Button::new("Save")).on_hover_text("Cmd+S").clicked()
                && let Some(m) = self.save()
            {
                self.console_log.push(m);
            }
            if ui
                .add_enabled(has_doc, egui::Button::new("Save as…"))
                .on_hover_text("Cmd+Shift+S")
                .clicked()
                && let Some(m) = self.save_as()
            {
                self.console_log.push(m);
            }
            if ui.add_enabled(has_doc, egui::Button::new("Import…")).on_hover_text("Add the models of a .vox, XRAW, QB or SLAB6 file").clicked() {
                let m = self.import_command("import");
                self.console_log.push(m);
                if self.mesh_dirty {
                    self.refresh_document(state);
                }
            }
        });
        ui.add_enabled_ui(self.scene.is_some(), |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Export");
                for (name, _, hint) in exchange::FORMATS {
                    if ui.small_button(name).on_hover_text(hint).clicked() {
                        let m = self.export_command(&format!("export {name}"));
                        self.console_log.push(m);
                    }
                }
            });
        });
        self.recent_ui(ui, state);
        ui.label(&self.status);

        // Read everything needed up front, so no borrow of `self.scene` is
        // alive when a click calls back into `&mut self`.
        let summary = self
            .scene
            .as_ref()
            .map(|s| (s.name.clone(), s.doc.scene.instances.len(), s.doc.scene.total_voxels()));

        if let Some((name, placed, voxels)) = summary {
            ui.label(name);
            ui.label(format!("{placed} placed models"));
            ui.label(format!("{voxels} voxels"));
            ui.label(format!("{} triangles", self.scene_renderer.triangle_count()));
        }

        ui.separator();
        ui.horizontal(|ui| {
            if ui.selectable_label(!self.rendering, "Model").clicked() {
                self.rendering = false;
            }
            if ui.selectable_label(self.rendering, "Render").clicked() {
                self.rendering = true;
                self.path_tracer.reset();
            }
        });

        ui.label("Camera");
        self.camera_ui(ui);
        ui.horizontal(|ui| {
            ui.label("View");
            self.view_ui(ui);
        });
        if ui.small_button("Snapshot…").on_hover_text("The viewport as a PNG (the reference editor's Capture Image)").clicked() {
            let message = self.snap(None, state);
            self.console_log.push(message);
        }

        if self.rendering {
            ui.label(format!(
                "{} samples, {} emissive voxels",
                self.path_tracer.samples,
                self.path_tracer.emitter_count()
            ));
            if ui.button("Save image…").on_hover_text("The render as it is now, as PNG").clicked() {
                let stem = self.scene.as_ref().and_then(|s| {
                    std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned())
                });
                let dialog = rfd::FileDialog::new()
                    .add_filter("PNG image", &["png"])
                    .set_file_name(format!("{}.png", stem.unwrap_or_else(|| "render".to_owned())));
                if let Some(path) = dialog.save_file() {
                    let message = self.save_render(&path.to_string_lossy(), state);
                    self.console_log.push(message);
                }
            }
            self.render_settings_ui(ui, state);
            ui.separator();
        }

        self.palette_grid(ui);
        self.palette_files_ui(ui, state);
        self.palette_editor_ui(ui, state);

        ui.separator();
        ui.heading("Brush");
        ui.horizontal(|ui| {
            for (mode, name) in voxel_core::BrushMode::ALL {
                let on = !self.selecting && !self.moving && self.brush_mode == mode;
                if ui.selectable_label(on, capitalised(name)).clicked() {
                    self.brush_mode = mode;
                    self.selecting = false;
                    self.moving = false;
                }
            }
            if ui
                .selectable_label(self.selecting, "Select")
                .on_hover_text("Shift adds, Option subtracts. Box drags a marquee.")
                .clicked()
            {
                self.selecting = true;
                self.moving = false;
            }
            if ui
                .selectable_label(self.moving, "Move")
                .on_hover_text("Drag a model (or its selected group). Shift: up and down. Cmd: turn. X/Y/Z: quarter turn.")
                .clicked()
            {
                self.moving = true;
                self.selecting = false;
            }
        });
        ui.horizontal_wrapped(|ui| {
            for (shape, name) in voxel_core::BrushShape::ALL {
                ui.selectable_value(&mut self.brush_shape, shape, capitalised(name));
            }
        });
        self.brush_options_ui(ui);
        ui.horizontal(|ui| {
            ui.label("Mirror");
            for (axis, name) in ["x", "y", "z"].iter().enumerate() {
                if ui
                    .selectable_label(self.mirror[axis], *name)
                    .on_hover_text("Repeat strokes reflected through the model's centre")
                    .clicked()
                {
                    self.mirror[axis] = !self.mirror[axis];
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Axis");
            for (axis, name) in ["x", "y", "z"].iter().enumerate() {
                if ui
                    .selectable_label(self.axis_lock[axis], *name)
                    .on_hover_text("Strokes run through the whole model along this axis")
                    .clicked()
                {
                    self.axis_lock[axis] = !self.axis_lock[axis];
                }
            }
        });
        match self.brush_shape {
            voxel_core::BrushShape::Fill if self.selecting || self.brush_mode != voxel_core::BrushMode::Attach => {
                use voxel_core::brush::Region;
                ui.horizontal_wrapped(|ui| {
                    ui.label("Region");
                    let wide = matches!(self.region, Region::Connected { wide: true, .. });
                    for (label, rule, hint) in [
                        ("Same colour", Region::SameColour, "Every voxel of the clicked colour"),
                        ("Colour", Region::Connected { by_colour: true, wide }, "Connected voxels of the clicked colour"),
                        ("Geo", Region::Connected { by_colour: false, wide }, "Connected voxels of any colour"),
                    ] {
                        let on = match (self.region, rule) {
                            (Region::SameColour, Region::SameColour) => true,
                            (Region::Connected { by_colour: a, .. }, Region::Connected { by_colour: b, .. }) => a == b,
                            _ => false,
                        };
                        if ui.selectable_label(on, label).on_hover_text(hint).clicked() {
                            self.region = rule;
                        }
                    }
                    if let Region::Connected { by_colour, wide } = self.region {
                        for (label, w) in [("4", false), ("8", true)] {
                            if ui
                                .selectable_label(wide == w, label)
                                .on_hover_text("Through faces only (4), or edges and corners too (8)")
                                .clicked()
                            {
                                self.region = Region::Connected { by_colour, wide: w };
                            }
                        }
                    }
                });
            }
            voxel_core::BrushShape::Shader => {
                let name = self.xs.chosen_name().unwrap_or_else(|| "none: choose one under Scripts".to_owned());
                ui.label(format!("Script: {name}"))
                    .on_hover_text("A click runs it over the model; the brush mode attaches, erases or paints its result");
            }
            voxel_core::BrushShape::Pattern => self.pattern_ui(ui),
            _ => {}
        }
        ui.horizontal(|ui| {
            ui.add(egui::Slider::new(&mut self.brush_color, 1..=255).text("colour"));
            if ui
                .selectable_label(self.picking, "Pick")
                .on_hover_text("Take the colour of the next voxel clicked (I)")
                .clicked()
            {
                self.picking = !self.picking;
            }
        });

        // Show the colour the brush will actually paint.
        if let Some(loaded) = &self.scene {
            let c = loaded.doc.palette.color(self.brush_color);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 16.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 2.0, egui::Color32::from_rgb(c.r, c.g, c.b));
        }

        ui.separator();
        ui.heading("Project");
        self.project_ui(ui, state);

        ui.separator();
        ui.heading("Scripts");
        self.scripts_ui(ui, state);

        ui.separator();
        ui.heading("Scene");
        self.scene_ui(ui);
        ui.separator();
        ui.heading("Animation");
        self.animation_ui(ui);
        ui.separator();
        ui.heading("Layers");
        self.layers_ui(ui);
        ui.separator();
        self.selection_ui(ui);
        ui.separator();
        ui.heading("Edit");
        self.edit_ui(ui);
        ui.separator();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.history.can_undo(), egui::Button::new("Undo"))
                .clicked()
            {
                self.undo(state);
            }
            if ui
                .add_enabled(self.history.can_redo(), egui::Button::new("Redo"))
                .clicked()
            {
                self.redo(state);
            }
        });
        ui.label(format!("{} undo steps", self.history.undo_depth()));



        ui.separator();
        self.console_ui(ui, state);
        self.mcp_panel(ui);

        ui.separator();
        ui.label("Drag on the model to edit, elsewhere (or right-drag) to orbit.");
        ui.label("Shift+drag or middle-drag to pan, scroll to zoom.");
        ui.label("Option+click selects a group (again: one level in).");
        ui.label("F frame, 1-7 views, O ortho, Cmd+Z undo.");
        ui.separator();
        ui.label("Samples");

        let mut pick = None;
        egui::ScrollArea::vertical().max_height(220.0).id_salt("samples").show(ui, |ui| {
            if let Ok(entries) = std::fs::read_dir(sample_dir()) {
                let mut paths: Vec<_> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "vox"))
                    .collect();
                paths.sort();
                for path in paths {
                    let label = path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let selected = self.scene.as_ref().is_some_and(|s| s.name == label);
                    if ui.selectable_label(selected, label).clicked() {
                        pick = Some(path.to_string_lossy().into_owned());
                    }
                }
            }
        });
        if let Some(path) = pick {
            self.request(files::Discarding::Open(Some(path)), &ctx, state);
        }
    }

    /// The 256 palette entries as a clickable grid.
    fn palette_grid(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &self.scene else { return };

        const COLS: usize = 16;
        const CELL: f32 = 13.0;
        let size = egui::vec2(COLS as f32 * CELL, (256 / COLS) as f32 * CELL);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

        let painter = ui.painter();
        for i in 0..256usize {
            let (cx, cy) = (i % COLS, i / COLS);
            let cell = egui::Rect::from_min_size(
                rect.min + egui::vec2(cx as f32 * CELL, cy as f32 * CELL),
                egui::vec2(CELL - 1.0, CELL - 1.0),
            );
            let c = loaded.doc.palette.color(i as u8);
            painter.rect_filled(cell, 1.0, egui::Color32::from_rgb(c.r, c.g, c.b));
            if i as u8 == self.brush_color {
                painter.rect_stroke(
                    cell,
                    1.0,
                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                    egui::StrokeKind::Inside,
                );
            }
        }

        if let (true, Some(pos)) = (response.clicked(), response.interact_pointer_pos()) {
            let local = pos - rect.min;
            let (cx, cy) = ((local.x / CELL) as usize, (local.y / CELL) as usize);
            if cx < COLS {
                let index = cy * COLS + cx;
                if index < 256 {
                    self.brush_color = index as u8;
                }
            }
        }
    }

    /// Text command console, in the spirit of the editor's own F2 console.
    fn console_ui(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        ui.heading("Console");

        let entered = ui
            .add(
                egui::TextEdit::singleline(&mut self.console_input)
                    .hint_text("help")
                    .desired_width(f32::INFINITY),
            )
            .lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter));

        if entered {
            let line = std::mem::take(&mut self.console_input);
            let ctx = ui.ctx().clone();
            self.run_command(&line, &ctx, state);
        }

        egui::ScrollArea::vertical()
            .max_height(80.0)
            .id_salt("console log")
            .show(ui, |ui| {
                for entry in self.console_log.iter().rev().take(20) {
                    ui.label(entry);
                }
            });
    }

    fn run_command(&mut self, line: &str, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        if line.trim().is_empty() {
            return;
        }
        // A long line (an agent's list of voxels) is shortened in the log.
        let shown: String = line.chars().take(120).collect();
        let more = if shown.len() < line.len() { " …" } else { "" };
        self.console_log.push(format!("> {shown}{more}"));

        if line.split_whitespace().next() == Some("env") {
            let message = self.env_command(line, state);
            self.console_log.push(message);
            return;
        }

        // the reference editor's names for commands the editor already had.
        let renamed;
        let line = match line.split_whitespace().next() {
            Some("shrink") => "crop",
            Some("flip") => {
                renamed = line.trim_start().replacen("flip", "mirror", 1);
                renamed.as_str()
            }
            Some("rot90") => {
                renamed = line.trim_start().replacen("rot90", "rotate", 1);
                renamed.as_str()
            }
            _ => line,
        };

        if line.split_whitespace().next().is_some_and(|w| model_edit::WORDS.contains(&w)) {
            let message = self.edit_command(line);
            self.console_log.push(message);
            if self.mesh_dirty {
                self.revalidate_selection();
                self.refresh_document(state);
            }
            return;
        }

        if matches!(
            line.split_whitespace().next(),
            Some(
                "select" | "delete" | "paint" | "copy" | "cut" | "paste" | "move" | "rotate"
                    | "mirror" | "crop" | "resize"
            )
        ) {
            let message = self.select_command(line, ctx);
            self.console_log.push(message);
            if self.mesh_dirty {
                self.revalidate_selection();
                self.refresh_document(state);
            }
            return;
        }

        // Through the app, not the console's own history calls, so the
        // outline selection is dropped as it is for Cmd+Z.
        if matches!(line.trim(), "undo" | "redo") {
            let before = self.history.undo_depth();
            if line.trim() == "undo" {
                self.undo(state);
            } else {
                self.redo(state);
            }
            let moved = self.history.undo_depth() != before;
            let message = match (line.trim(), moved) {
                ("undo", true) => "undone",
                ("undo", false) => "nothing to undo",
                (_, true) => "redone",
                _ => "nothing to redo",
            };
            self.console_log.push(message.to_owned());
            return;
        }

        if matches!(line.split_whitespace().next(), Some("group" | "ungroup" | "node" | "layer")) {
            let message = if line.starts_with("layer") {
                self.layer_command(line)
            } else {
                self.group_command(line)
            };
            self.console_log.push(message);
            if self.mesh_dirty {
                self.refresh_document(state);
            }
            return;
        }

        if line.split_whitespace().next() == Some("anim") {
            let message = self.anim_command(line);
            self.console_log.push(message);
            if self.mesh_dirty {
                self.refresh_document(state);
            }
            return;
        }

        if let ["render", "save", path] = line.split_whitespace().collect::<Vec<_>>().as_slice() {
            let message = self.save_render(path, state);
            self.console_log.push(message);
            return;
        }

        if line.split_whitespace().next() == Some("camera") {
            let message = self.camera_command(line);
            self.console_log.push(message);
            return;
        }

        if line.split_whitespace().next() == Some("instance") {
            let message = self.instance_command(line);
            self.console_log.push(message);
            if self.mesh_dirty {
                self.refresh_document(state);
            }
            return;
        }

        if line.split_whitespace().next() == Some("transform") {
            let message = self.transform_command(line);
            self.console_log.push(message);
            if self.mesh_dirty {
                self.refresh_document(state);
            }
            return;
        }

        if line.split_whitespace().next() == Some("view") {
            let message = self.view_command(line);
            self.console_log.push(message);
            if self.mesh_dirty {
                self.refresh_document(state);
            }
            return;
        }

        if line.split_whitespace().next() == Some("snap") && line.split_whitespace().nth(1) == Some("window") {
            // the reference editor's Capture Window: the whole window, panels and
            // all, as egui draws it; written when the screenshot arrives.
            let path = match line.split_whitespace().nth(2) {
                Some(p) => p.to_owned(),
                None => match rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("window.png").save_file() {
                    Some(p) => p.to_string_lossy().into_owned(),
                    None => {
                        self.console_log.push("snapshot cancelled".to_owned());
                        return;
                    }
                },
            };
            // Asked for from the next drawn frame: a request sent before the
            // window has drawn (a startup line) would be lost.
            self.window_snap = Some(path.clone());
            self.window_snap_sent = false;
            self.console_log.push(format!("{path} will be saved with the next frame"));
            return;
        }

        if line.split_whitespace().next() == Some("snap") {
            let message = self.snap(line.split_whitespace().nth(1), state);
            self.console_log.push(message);
            return;
        }

        if line.split_whitespace().next() == Some("export") {
            let message = self.export_command(line);
            self.console_log.push(message);
            return;
        }

        if line.split_whitespace().next() == Some("import") {
            let message = self.import_command(line);
            self.console_log.push(message);
            if self.mesh_dirty {
                self.refresh_document(state);
            }
            return;
        }

        if matches!(line.split_whitespace().next(), Some("palette" | "material")) {
            let message = self.palette_command(line, state);
            self.console_log.push(message);
            return;
        }

        if line.split_whitespace().next() == Some("project") {
            let message = self.project_command(line, ctx, state);
            self.console_log.push(message);
            return;
        }

        if line.split_whitespace().next() == Some("pattern") {
            let message = self.pattern_command(line);
            self.console_log.push(message);
            return;
        }

        if matches!(line.split_whitespace().next(), Some("brush" | "color")) {
            let message = self.brush_command(line);
            self.console_log.push(message);
            return;
        }

        if matches!(line.split_whitespace().next(), Some("save" | "open" | "new" | "recover")) {
            let message = self.file_command(line, ctx, state);
            if !message.is_empty() {
                self.console_log.push(message);
            }
            return;
        }

        if line.split_whitespace().next() == Some("xs") {
            let message = self.xs_command(line, state);
            self.console_log.push(message);
            return;
        }

        if line.split_whitespace().next() == Some("gpu") {
            let message = self.run_gpu_command(line, state);
            self.console_log.push(message);
            return;
        }

        // Reach the fields directly rather than through a `&mut self`
        // helper, so the document and the history can be borrowed together.
        let mut changed = false;
        let mut message = None;
        let model = self.selected_model();
        if let (Some(loaded), Some(model)) = (self.scene.as_mut(), model) {
            match console::execute(line, &mut loaded.doc, model, &mut self.history) {
                console::Outcome::Ok { message: m, changed: c } => {
                    changed = c;
                    if !m.is_empty() {
                        message = Some(m);
                    }
                }
                console::Outcome::Error(e) => message = Some(format!("error: {e}")),
            }
        }
        if let Some(m) = message {
            self.console_log.push(m);
        }
        if changed {
            self.refresh_document(state);
        }
    }

    /// Loads the environment map the current settings name, if it is not
    /// already resident. A missing or unreadable map turns image lighting
    /// off with a message rather than rendering a black sky.
    fn ensure_env_map(&mut self, state: &egui_wgpu::RenderState) -> Option<String> {
        if self.environment.atmosphere_enabled {
            // Cheap enough to rebake whenever it is asked for: the sky is
            // smooth, so a small map is plenty.
            let started = std::time::Instant::now();
            let map = self.environment.atmosphere_map(128, 64);
            self.path_tracer.set_computed_sky(&state.device, "atmosphere", &map);
            let message = format!("sky: atmosphere ({:.0?})", started.elapsed());
            eprintln!("[voxferro] {message}");
            return Some(message);
        }
        if !self.environment.ibl_enabled {
            return None;
        }
        let wanted = self.environment.ibl_path.clone();
        if self.path_tracer.environment_map_name() == Some(wanted.as_str()) {
            return None;
        }
        let result = find_env_map(&wanted)
            .ok_or_else(|| format!("environment map {wanted} not found"))
            .and_then(|path| std::fs::read(&path).map_err(|e| e.to_string()))
            .and_then(|bytes| voxel_core::EnvMap::parse_hdr(&bytes).map_err(|e| e.to_string()));

        match result {
            Ok(map) => {
                self.path_tracer.set_environment_map(&state.device, &wanted, &map);
                let message = format!("environment {wanted} ({}x{})", map.width, map.height);
                eprintln!("[voxferro] {message}");
                Some(message)
            }
            Err(e) => {
                self.environment.ibl_enabled = false;
                eprintln!("[voxferro] {e}; using the uniform sky");
                Some(format!("{e}; using the uniform sky"))
            }
        }
    }

    /// `gpu <shape> [colour] [radius] [thickness] [replace]`
    ///
    /// Runs the kernel, then feeds the result through the ordinary command
    /// stack, so a GPU operation is as undoable as a mouse click.
    fn run_gpu_command(&mut self, line: &str, state: &egui_wgpu::RenderState) -> String {
        let args: Vec<&str> = line.split_whitespace().skip(1).collect();
        let Some(shape_name) = args.first() else {
            return format!("gpu needs a shape: {}", compute::Shape::ALL.join(", "));
        };
        let Some(shape) = compute::Shape::parse(shape_name) else {
            return format!(
                "unknown shape '{shape_name}' (try: {})",
                compute::Shape::ALL.join(", ")
            );
        };

        let color = args
            .get(1)
            .and_then(|a| a.parse::<u8>().ok())
            .unwrap_or(self.brush_color);

        let model = self.selected_model();
        let (Some(loaded), Some(model)) = (self.scene.as_mut(), model) else {
            return "no model loaded".to_owned();
        };
        let volume = &loaded.doc.scene.models[model];

        let [sx, sy, sz] = volume.size();
        let smallest = sx.min(sy).min(sz) as f32;
        let radius = args
            .get(2)
            .and_then(|a| a.parse::<f32>().ok())
            .unwrap_or(smallest * 0.4);
        let thickness = args
            .get(3)
            .and_then(|a| a.parse::<f32>().ok())
            .unwrap_or(smallest * 0.12);
        let replace = args.get(4).is_some_and(|a| *a == "replace");

        let started = std::time::Instant::now();
        let Some(result) = self.voxel_compute.run(
            &state.device,
            &state.queue,
            volume,
            compute::Op {
                shape,
                color,
                radius,
                thickness,
                replace,
            },
        ) else {
            return "gpu kernel produced nothing".to_owned();
        };
        let elapsed = started.elapsed();

        // Turn the raw result into an undoable edit.
        let mut writes = Vec::new();
        let mut i = 0usize;
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let new = result[i];
                    if volume.get(x, y, z) != new {
                        writes.push((glam::IVec3::new(x as i32, y as i32, z as i32), new));
                    }
                    i += 1;
                }
            }
        }

        let count = writes.len();
        self.history.end_gesture();
        self.history.set_voxels(&mut loaded.doc, model, writes);
        self.history.end_gesture();
        self.mesh_dirty = true;

        format!("gpu {shape_name}: {count} voxels in {elapsed:.1?}")
    }

    /// Writes a window screenshot a `snap window` asked for, once egui
    /// delivers it.
    fn take_window_snap(&mut self, ctx: &egui::Context) {
        if self.window_snap.is_none() {
            return;
        }
        if !self.window_snap_sent {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.window_snap_sent = true;
            ctx.request_repaint();
            return;
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        let Some(image) = image else {
            // The capture comes back with a later frame: keep drawing.
            ctx.request_repaint();
            return;
        };
        let Some(path) = self.window_snap.take() else { return };
        let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
        let [w, h] = image.size;
        let message = match std::fs::write(&path, voxel_core::export::png_rgba(w as u32, h as u32, &rgba)) {
            Ok(()) => format!("saved {path} ({w}x{h}, the window)"),
            Err(e) => format!("error: could not write {path}: {e}"),
        };
        eprintln!("[voxferro] {message}");
        self.console_log.push(message);
    }

    /// the reference editor's Capture Image: the viewport as it is -- the editor's
    /// view in Model mode, the render in Render mode -- as a PNG. Without a
    /// path, asks where.
    fn snap(&mut self, path: Option<&str>, state: &egui_wgpu::RenderState) -> String {
        let path = match path {
            Some(p) => p.to_owned(),
            None => match rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("snapshot.png").save_file() {
                Some(p) => p.to_string_lossy().into_owned(),
                None => return "snapshot cancelled".to_owned(),
            },
        };
        if self.rendering {
            return self.save_render(&path, state);
        }
        match self.scene_renderer.read_frame(&state.device, &state.queue) {
            Some((w, h, rgba)) => match std::fs::write(&path, voxel_core::export::png_rgba(w, h, &rgba)) {
                Ok(()) => format!("saved {path} ({w}x{h})"),
                Err(e) => format!("error: could not write {path}: {e}"),
            },
            // Nothing drawn yet (a script's first lines run before the first
            // frame): taken after the next one.
            None => {
                self.pending_snap = Some(path.clone());
                format!("{path} will be saved after the next frame")
            }
        }
    }

    /// Writes the path traced image as it is now -- tone mapped, as shown
    /// -- to a PNG.
    fn save_render(&mut self, path: &str, state: &egui_wgpu::RenderState) -> String {
        if !self.rendering || self.path_tracer.samples == 0 {
            return "error: nothing rendered: switch to Render first".to_owned();
        }
        let Some((w, h, rgba)) = self.path_tracer.read_frame(&state.device, &state.queue) else {
            return "error: nothing rendered yet".to_owned();
        };
        match std::fs::write(path, voxel_core::export::png_rgba(w, h, &rgba)) {
            Ok(()) => format!("saved {path} ({w}x{h}, {} samples)", self.path_tracer.samples),
            Err(e) => format!("error: could not write {path}: {e}"),
        }
    }

    /// Draws the scene into its offscreen target at `size` pixels: one
    /// more sample of the path tracer in Render mode, else the raster view
    /// with its guide lines. Returns the egui texture holding the image.
    /// Painting it is the caller's: the viewport shows it, and an agent's
    /// capture reads it back while the window is hidden.
    pub fn draw_scene(&mut self, state: &egui_wgpu::RenderState, size: [u32; 2]) -> Option<egui::TextureId> {
        self.last_view_size = size;
        let view_proj = self.camera.view_proj(size[0] as f32 / size[1] as f32);

        if self.mesh_dirty {
            self.remesh(state);
        }

        if self.rendering {
            let basis = self.camera.basis();
            let offset = self.volume_offset();
            let mut egui_renderer = state.renderer.write();
            return self.path_tracer.render(
                &state.device,
                &state.queue,
                &mut egui_renderer,
                size,
                self.camera.eye(),
                basis,
                self.camera.fov_y,
                self.camera.ortho().then(|| self.camera.half_height()),
                // In focus: the point the camera orbits, which framing and
                // panning put on the subject.
                self.camera.distance,
                &self.environment,
                offset,
            );
        }

        let guides = self.model_guides();
        if guides != self.guide_lines {
            self.scene_renderer.set_lines(&state.device, &guides);
            self.guide_lines = guides;
        }

        let mut egui_renderer = state.renderer.write();
        Some(self.scene_renderer.render(
            &state.device,
            &state.queue,
            &mut egui_renderer,
            size,
            view_proj,
            self.light_dir,
            Vec3::new(0.19, 0.18, 0.26), // the editor's own default ambient
            [0.16, 0.16, 0.17],
            self.view.edges,
            self.view.shadow,
        ))
    }

    fn viewport(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

        let ppp_early = ui.ctx().pixels_per_point();
        let view_size = egui::vec2(rect.width(), rect.height());
        let _ = ppp_early;

        // Headless runs (--screenshot, --render) must not react to the mouse:
        // a stray scroll over the window during a long render zooms the
        // camera, and the tracer then faithfully renders the wrong view.
        let headless = self.screenshot.is_some() || self.pending_samples > 0;

        let aspect = view_size.x / view_size.y.max(1.0);
        let ray_at = |camera: &OrbitCamera, pos: egui::Pos2| {
            let local = pos - rect.min;
            edit::ray_through_pixel(
                camera.view_proj(aspect),
                glam::Vec2::new(view_size.x, view_size.y),
                glam::Vec2::new(local.x, local.y),
            )
        };

        if !self.scripted_strokes.is_empty() {
            for (from, to) in std::mem::take(&mut self.scripted_strokes) {
                let at = |f: egui::Vec2| rect.min + f * view_size;
                if self.picking {
                    self.begin_stroke(ray_at(&self.camera, at(from)));
                } else if self.transform_tool.is_some() && self.begin_transform(at(from)) {
                    // A scripted drag keeps its transform, as Enter would.
                    self.update_transform(at(to), false);
                    let message = self.finish_transform(true);
                    eprintln!("[voxferro] {message}");
                } else if self.moving {
                    if self.begin_move(ray_at(&self.camera, at(from)), at(from).x, egui::Modifiers::default()) {
                        self.update_move(ray_at(&self.camera, at(to)), at(to).x);
                        self.finish_move();
                    }
                } else if self.selecting {
                    let mods = egui::Modifiers::default();
                    if self.begin_select(ray_at(&self.camera, at(from)), at(from), mods) {
                        self.update_select(ray_at(&self.camera, at(to)), at(to));
                        self.finish_select(self.camera.view_proj(aspect), rect);
                    }
                } else if self.begin_stroke(ray_at(&self.camera, at(from))) {
                    self.update_stroke(ray_at(&self.camera, at(to)));
                    self.finish_stroke();
                }
            }
            eprintln!(
                "[voxferro] scripted strokes done; selected instance {}, {} undo steps",
                self.selected,
                self.history.undo_depth()
            );
        }

        // The primary button edits when it goes down on a voxel and orbits
        // when it goes down on empty space; the secondary and middle buttons
        // always orbit. In Render mode every drag orbits.
        let (pressed, down, released, delta, secondary, middle, modifiers) = ui.input(|i| {
            let p = &i.pointer;
            (
                p.primary_pressed(),
                p.primary_down(),
                p.primary_released(),
                p.delta(),
                p.secondary_down(),
                p.middle_down(),
                i.modifiers,
            )
        });
        let pointer = response.hover_pos();
        // A click on the view cube is the cube's alone.
        let on_cube = self.view_cube_shown && pointer.is_some_and(|p| view_cube::cube_rect(rect).contains(p));
        if !headless {
            if pressed && response.hovered() && !on_cube {
                let hit = !self.rendering
                    && pointer.is_some_and(|pos| {
                        let ray = ray_at(&self.camera, pos);
                        if self.picking {
                            // The eyedropper wins over any tool.
                            self.begin_stroke(ray)
                        } else if self.transform_tool.is_some() && self.begin_transform(pos) {
                            true
                        } else if self.selecting {
                            self.begin_select(ray, pos, modifiers)
                        } else if modifiers.alt {
                            // Option+click picks groups instead of painting
                            // (Option subtracts only in the Select tool).
                            self.select_group_at(ray)
                        } else if self.moving {
                            self.begin_move(ray, pos.x, modifiers)
                        } else {
                            self.begin_stroke(ray)
                        }
                    });
                self.orbiting = !hit;
            }
            if down
                && self.transform_drag.as_ref().is_some_and(|d| d.dragging)
                && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            {
                self.update_transform(pos, modifiers.shift);
            }
            if down && (self.stroke.is_some() || self.select_drag.is_some() || self.move_drag.is_some())
                && let Some(pos) = ui.input(|i| i.pointer.interact_pos())
            {
                let ray = ray_at(&self.camera, pos);
                if self.move_drag.is_some() {
                self.update_move(ray, pos.x);
            } else if self.select_drag.is_some() {
                        self.update_select(ray, pos);
                    } else {
                        self.update_stroke(ray);
                    }
                }
            // The factor turns pixels into a comfortable radians-per-pixel
            // rate.
            // Shift turns an orbit into a pan, and the middle button always
            // pans, as in the editor (and usable from a trackpad).
            let orbit = down && self.orbiting || secondary && response.hovered();
            let pan = middle && response.hovered() || orbit && modifiers.shift;
            if delta != egui::Vec2::ZERO && (orbit || pan) {
                if pan {
                    self.camera.pan(delta.x, delta.y, view_size.y);
                } else {
                    self.camera.orbit(delta.x * 0.01, delta.y * 0.01);
                }
                self.path_tracer.reset();
            }
            if released {
                if let Some(drag) = &mut self.transform_drag {
                    drag.dragging = false;
                }
                self.finish_move();
                self.finish_stroke();
                self.finish_select(self.camera.view_proj(aspect), rect);
                self.orbiting = false;
            }
        }

        if !headless && response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(1.0 - scroll * 0.002);
                self.path_tracer.reset();
            }
        }

        // Render at physical pixels, not logical points, or the image is soft
        // on a Retina display.
        let ppp = ui.ctx().pixels_per_point();
        let size = [
            ((rect.width() * ppp) as u32).max(1),
            ((rect.height() * ppp) as u32).max(1),
        ];
        let id = self.draw_scene(state, size);
        if let Some(path) = self.pending_snap.take() {
            let message = self.snap(Some(&path), state);
            self.console_log.push(message);
        }

        if self.rendering {
            if let Some(id) = id {
                ui.painter().image(
                    id,
                    rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                // Keep sampling: the image refines frame by frame.
                ui.ctx().request_repaint();
            }
            return;
        }

        // Outline the cell the brush would act on.
        let hover = if headless || self.stroke.is_some() || self.orbiting || self.selecting || self.moving {
            None
        } else {
            pointer.and_then(|pos| self.hover_cell(ray_at(&self.camera, pos)))
        };

        let Some(texture_id) = id else { return };
        ui.painter().image(
            texture_id,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );

        if let Some(drag) = &self.select_drag
            && self.brush_shape == voxel_core::BrushShape::Box
        {
            let marquee = egui::Rect::from_two_pos(drag.from, drag.to);
            ui.painter().rect_stroke(
                marquee,
                0.0,
                egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 180, 255)),
                egui::StrokeKind::Inside,
            );
        }

        // The selected group's box, so a group picked with Option+click (or in
        // the outline) shows what it holds.
        let group_box = self.move_target_bounds().or_else(|| self.bounds_group());
        let vp = self.camera.view_proj(aspect);
        let project = |p: Vec3| {
            let c = vp * p.extend(1.0);
            let ndc = c.truncate() / c.w.max(1e-6);
            rect.min + egui::vec2((ndc.x + 1.0) * 0.5 * view_size.x, (1.0 - ndc.y) * 0.5 * view_size.y)
        };
        if let Some((lo, hi)) = group_box {
            let stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 180, 255));
            let corner = |i: usize| {
                Vec3::new(
                    if i & 1 == 0 { lo.x } else { hi.x },
                    if i & 2 == 0 { lo.y } else { hi.y },
                    if i & 4 == 0 { lo.z } else { hi.z },
                )
            };
            for (a, b) in CUBE_EDGES {
                ui.painter().line_segment([project(corner(a)), project(corner(b))], stroke);
            }
        }

        if let Some(corners) = hover {
            let stroke = egui::Stroke::new(1.5, egui::Color32::from_rgb(255, 230, 120));
            for (a, b) in CUBE_EDGES {
                ui.painter().line_segment([project(corners[a]), project(corners[b])], stroke);
            }
        }
        if !headless {
            self.view_cube(ui, rect);
        }
    }
}

/// Arguments that are not flags or a flag's values, i.e. the model path.
///
/// Searching every argument for ".vox" would also find the path inside
/// `--run "save out.vox"`.
fn positional_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let skip = match args[i].as_str() {
            "--run" | "--script" | "--screenshot" | "--icon" | "--render" => 1,
            "--mcp" if args.get(i + 1).is_some_and(|p| p.parse::<u16>().is_ok()) => 1,
            "--camera" | "--click" => 2,
            "--drag" => 4,
            a if a.starts_with("--") => 0,
            a => {
                out.push(a.to_owned());
                0
            }
        };
        i += 1 + skip;
    }
    out
}

/// The 12 edges of a box whose corners are numbered by bits x, y, z.
const CUBE_EDGES: [(usize, usize); 12] =
    [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];

fn capitalised(name: &str) -> String {
    let mut chars = name.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// What is drawn, traced, picked and exported: a snapshot taken before
/// borrowing the scene, so filters can run while the scene is borrowed.
pub struct Visibility {
    hidden_layers: Vec<i32>,
}

impl Visibility {
    pub fn shows(&self, instance: &voxel_core::Instance) -> bool {
        !instance.hidden && !self.hidden_layers.contains(&instance.layer)
    }
}

impl App {
    pub fn visibility(&self) -> Visibility {
        let hidden_layers = self
            .scene
            .as_ref()
            .map(|s| s.doc.layers.iter().filter(|l| l.is_hidden()).map(|l| l.id).collect())
            .unwrap_or_default();
        Visibility { hidden_layers }
    }
}

impl eframe::App for App {
    /// Runs before every `ui`, and also while the window is hidden or
    /// covered, whenever something asks for a repaint -- which `ui` is not:
    /// eframe skips the egui pass for a window nobody can see. Agents' MCP
    /// requests are taken here so they are answered while the editor sits
    /// behind the terminal they were sent from.
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let Some(state) = frame.wgpu_render_state().cloned() else { return };
        // Files opened from Finder.
        #[cfg(target_os = "macos")]
        for path in macos_open::take() {
            self.console_log.push(format!("opening {path}"));
            self.request(files::Discarding::Open(Some(path)), ctx, &state);
        }
        self.mcp_poll(ctx, &state);
        // Hidden, nothing draws the view, so a pending capture draws it
        // here at the size it last had; a render, one sample per call.
        let hidden = ctx.input(|i| i.viewport().visible() == Some(false));
        if hidden && self.mcp_capture.is_some() {
            self.draw_scene(&state, self.last_view_size);
            self.mcp_finish_capture(ctx, &state);
        }
        // A headless render must not stall behind another window either.
        let headless = self.pending_samples > 0 || self.screenshot.is_some();
        if hidden && headless && self.scripted_strokes.is_empty() {
            self.draw_scene(&state, self.last_view_size);
            ctx.request_repaint();
            self.headless_capture(ctx, &state);
        }
    }

    /// Draws one frame.
    ///
    /// Note the signature: as of eframe 0.36 the `App` trait takes a `&mut Ui`
    /// and eframe owns the central panel, replacing the older
    /// `update(&mut self, ctx: &Context, ..)` that built its own panel. Most
    /// tutorials online still show the old shape.
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let Some(state) = frame.wgpu_render_state().cloned() else {
            ui.label("no wgpu render state");
            return;
        };

        // egui 0.36 dropped SidePanel for a generic Panel with side
        // constructors, and sizes it with default_size rather than
        // default_width.
        // Cmd+Z / Cmd+Shift+Z, matching the platform convention. egui maps
        // `command` to Cmd on macOS and Ctrl elsewhere.
        let (undo, redo) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Z) && i.modifiers.command && !i.modifiers.shift,
                (i.key_pressed(egui::Key::Z) && i.modifiers.command && i.modifiers.shift)
                    || (i.key_pressed(egui::Key::Y) && i.modifiers.command),
            )
        });
        if undo {
            self.undo(&state);
        }
        if redo {
            self.redo(&state);
        }

        let ctx = ui.ctx().clone();
        self.file_input(&ctx, &state);
        self.selection_input(&ctx);
        self.camera_input(&ctx);
        self.transform_input(&ctx);
        self.take_window_snap(&ctx);
        self.view_input(&ctx);
        self.brush_input(&ctx);
        if !self.view.background && self.meshed_for != Some(self.selected) {
            self.mesh_dirty = true;
        }
        self.move_input(&ctx);
        self.animate(&ctx);
        self.confirm_ui(&ctx, &state);
        self.project_confirm_ui(&ctx, &state);
        self.mcp_confirm_ui(&ctx, &state);
        let title = self.title();
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }

        egui::Panel::left("controls")
            .default_size(250.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("controls scroll")
                    .show(ui, |ui| self.controls(ui, &state));
            });

        egui::CentralPanel::default().show(ui, |ui| self.viewport(ui, &state));
        self.export_step(&ctx, &state);
        self.mcp_finish_capture(&ctx, &state);

        // Releasing the button ends whatever gesture was being merged: a
        // colour or material slider drag, as well as a stroke.
        if ui.input(|i| i.pointer.any_released()) {
            self.history.end_gesture();
        }

        self.headless_capture(&ctx, &state);
    }
}

impl App {
    /// The end of a headless run (`--render`, `--screenshot`, `--icon`):
    /// once the sample budget is reached, writes the image and exits.
    fn headless_capture(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        // Reads back the 3D target. egui's own ViewportCommand::Screenshot
        // would also capture the panels, but this eframe/wgpu backend never
        // delivers the resulting event here, so the viewport is what we can
        // verify automatically.
        // In --render mode, wait for the sample budget before capturing.
        if self.pending_samples > 0 {
            if self.path_tracer.samples < self.pending_samples {
                ctx.request_repaint();
                return;
            }
            eprintln!("[voxferro] path traced {} samples", self.path_tracer.samples);
            self.pending_samples = 0;
        }

        if let Some(path) = self.screenshot.take() {
            let frame = if self.rendering {
                self.path_tracer.read_frame(&state.device, &state.queue)
            } else {
                self.scene_renderer.read_frame(&state.device, &state.queue)
            };
            let ok = match frame {
                Some((w, h, rgba)) => match if self.write_icon {
                    icon::write(&path, w, h, &rgba)
                } else {
                    image_out::write_bmp(&path, w, h, &rgba)
                } {
                    Ok(()) => {
                        eprintln!("[voxferro] wrote {path} ({w}x{h})");
                        true
                    }
                    Err(e) => {
                        eprintln!("[voxferro] could not write {path}: {e}");
                        false
                    }
                },
                None => {
                    eprintln!("[voxferro] nothing rendered yet; no screenshot");
                    false
                }
            };

            // Exit directly rather than asking the window to close. In
            // --render mode a repaint is always in flight, and AppKit tearing
            // down the winit view mid display-cycle throws an NSRangeException
            // (a Touch Bar KVO observer on `nextResponder`) and aborts -- after
            // the image is already written, so it looked like success here
            // and like a crash dialog to the user. Headless runs have nothing
            // left to save, so skipping the teardown loses nothing.
            std::process::exit(if ok { 0 } else { 1 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::positional_args;

    #[test]
    fn a_path_inside_a_command_is_not_the_model_path() {
        let args: Vec<String> = ["--run", "save out.vox", "--click", "0.5", "0.5", "in.vox", "--render-mode"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(positional_args(&args), vec!["in.vox".to_owned()]);
    }
}
