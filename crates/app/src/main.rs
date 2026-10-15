//! Entry point for the voxel editor.
//!
//! eframe owns the window and the event loop. The scene is rendered with wgpu
//! into an offscreen texture that egui then displays, which is what gives us a
//! depth buffer (see `renderer`). On Apple Silicon wgpu emits Metal directly,
//! so nothing goes through the deprecated OpenGL stack.

mod camera;
mod compute;
mod console;
mod edit;
mod image_out;
mod renderer;
mod scene;

use camera::OrbitCamera;
use eframe::{egui, egui_wgpu};
use glam::Vec3;
use renderer::SceneRenderer;
use voxel_core::{History, Mesh};

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_title("Voxferro"),
        ..Default::default()
    };

    eframe::run_native(
        "Voxferro",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

/// Where the bundled sample models live, resolved at compile time so
/// `cargo run` works from any directory.
const ASSET_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/vox");

struct App {
    backend: String,
    camera: OrbitCamera,
    scene: Option<scene::LoadedScene>,
    /// Owned here rather than in egui's `callback_resources`: rendering needs
    /// `&mut` on egui's own renderer at the same time (to register the target
    /// texture), and both cannot be borrowed out of the same map at once.
    scene_renderer: SceneRenderer,
    voxel_compute: compute::VoxelCompute,
    status: String,
    /// Index into the scene's model list.
    shown_model: usize,
    light_dir: Vec3,
    history: History,
    /// Palette index the brush paints with.
    brush_color: u8,
    /// Removing instead of adding.
    erase: bool,
    /// Set when the scene changed and the GPU mesh is stale.
    mesh_dirty: bool,
    /// Per-layer visibility; the format defines eight.
    hidden_layers: [bool; 8],
    console_input: String,
    console_log: Vec<String>,
    /// When set, the app writes one frame of the whole window here and exits.
    /// Used to verify the UI without a human looking at the window, and to
    /// produce images for the README.
    screenshot: Option<String>,

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
            status: String::new(),
            shown_model: 0,
            light_dir: Vec3::new(0.3, 0.5, 0.8).normalize(),
            screenshot: None,
            history: History::default(),
            brush_color: 79,
            erase: false,
            mesh_dirty: false,
            hidden_layers: [false; 8],
            console_input: String::new(),
            console_log: vec![
                "type 'help', or 'gpu sphere' to run a compute kernel".to_owned(),
            ],
        };

        // Usage: voxferro [model.vox] [--run "<command>"] [--screenshot out.bmp]
        let args: Vec<String> = std::env::args().skip(1).collect();
        let screenshot = args
            .iter()
            .position(|a| a == "--screenshot")
            .and_then(|i| args.get(i + 1).cloned());
        let path = args
            .iter()
            .find(|a| a.ends_with(".vox"))
            .cloned()
            .unwrap_or_else(|| format!("{ASSET_DIR}/monu1.vox"));

        // Several --run flags may be given; they execute in order, which is
        // how a kernel and a save fit into one headless session.
        let startup_commands: Vec<String> = args
            .iter()
            .enumerate()
            .filter(|(_, a)| a.as_str() == "--run")
            .filter_map(|(i, _)| args.get(i + 1).cloned())
            .collect();

        app.screenshot = screenshot;
        app.load(state, &path);

        // Run before the first frame, so --screenshot captures the result.
        for command in &startup_commands {
            app.run_command(command, state);
            if app.mesh_dirty {
                app.remesh(state);
            }
        }
        if !startup_commands.is_empty() {
            for line in &app.console_log {
                eprintln!("[voxferro] {line}");
            }
        }
        app
    }

    fn load(&mut self, state: &egui_wgpu::RenderState, path: &str) {
        match scene::LoadedScene::load(path) {
            Ok(loaded) => {
                self.status = format!(
                    "{} models, {} placed, v{}",
                    loaded.scene.models.len(),
                    loaded.scene.instances.len(),
                    loaded.version
                );
                self.shown_model = 0;
                self.scene = Some(loaded);
                self.upload_current(state);
            }
            Err(e) => self.status = format!("failed to load {path}: {e}"),
        }
    }

    /// Meshes the whole scene and hands it to the GPU.
    ///
    /// Every instance is baked into one world-space mesh. Instanced draws
    /// would scale better for a scene that repeats one model many times, but
    /// a single buffer keeps the draw path trivial while the geometry fits.
    fn upload_current(&mut self, state: &egui_wgpu::RenderState) {
        // Disjoint field borrows: `scene` is read while `scene_renderer` is
        // mutated, which the borrow checker allows because they are separate
        // fields accessed directly rather than through a method on `self`.
        let Some(loaded) = &self.scene else { return };

        let hidden = self.hidden_layers;
        let mesh = Mesh::from_scene_where(&loaded.scene, |i| !is_hidden(&hidden, i.layer));
        eprintln!(
            "[voxferro] {}: {} instances, {} voxels, {} triangles",
            loaded.name,
            loaded.scene.instances.len(),
            loaded.scene.total_voxels(),
            mesh.triangle_count()
        );

        if let Some((min, max)) = mesh.bounds() {
            self.camera.frame_world_bounds(min, max);
        }
        self.scene_renderer.set_mesh(&state.device, &mesh);
        self.scene_renderer.set_palette(&state.queue, &loaded.palette);
    }

    /// Rebuilds the GPU mesh after an edit, leaving the camera alone.
    fn remesh(&mut self, state: &egui_wgpu::RenderState) {
        let Some(loaded) = &self.scene else { return };
        let hidden = self.hidden_layers;
        let mesh = Mesh::from_scene_where(&loaded.scene, |i| !is_hidden(&hidden, i.layer));
        self.scene_renderer.set_mesh(&state.device, &mesh);
        self.mesh_dirty = false;
    }

    /// Picks under the cursor and adds or removes a voxel.
    ///
    /// Edits target the first placed instance: a full editor would pick the
    /// nearest hit across every instance, which needs each one's own ray test.
    fn edit_at(&mut self, pixel: egui::Vec2, viewport: egui::Vec2, view_proj: glam::Mat4) {
        let Some(loaded) = &mut self.scene else { return };
        let Some(instance) = loaded.scene.instances.first().cloned() else {
            return;
        };
        let Some(volume) = loaded.scene.models.get_mut(instance.model_index) else {
            return;
        };

        let ray = edit::ray_through_pixel(
            view_proj,
            Vec3::new(viewport.x, viewport.y, 0.0).truncate(),
            Vec3::new(pixel.x, pixel.y, 0.0).truncate(),
        );

        let Some(hit) = edit::pick(volume, &instance.transform, ray, 10_000.0) else {
            return;
        };

        let (target, value) = if self.erase {
            (hit.voxel, 0)
        } else {
            (hit.adjacent(), self.brush_color)
        };

        self.history.set_voxels(volume, [(target, value)]);
        self.mesh_dirty = true;
    }

    fn undo(&mut self, state: &egui_wgpu::RenderState) {
        if self.apply_history(|h, v| h.undo(v)) {
            self.remesh(state);
        }
    }

    fn redo(&mut self, state: &egui_wgpu::RenderState) {
        if self.apply_history(|h, v| h.redo(v)) {
            self.remesh(state);
        }
    }

    /// Runs a history operation against the edited volume.
    ///
    /// A helper returning `&mut Volume` would borrow all of `self`, leaving
    /// `self.history` unreachable. Reaching the fields directly keeps the two
    /// borrows disjoint, and the inner scope ends them before `remesh` needs
    /// `self` again.
    fn apply_history(&mut self, op: impl FnOnce(&mut History, &mut voxel_core::Volume) -> bool) -> bool {
        let Some(loaded) = self.scene.as_mut() else {
            return false;
        };
        let Some(index) = loaded.scene.instances.first().map(|i| i.model_index) else {
            return false;
        };
        let Some(volume) = loaded.scene.models.get_mut(index) else {
            return false;
        };
        op(&mut self.history, volume)
    }

    fn controls(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        ui.heading("Voxferro");
        ui.label(&self.backend);
        ui.separator();
        ui.label(&self.status);

        // Read everything needed up front, so no borrow of `self.scene` is
        // alive when a click calls back into `&mut self`.
        let summary = self
            .scene
            .as_ref()
            .map(|s| (s.name.clone(), s.scene.instances.len(), s.scene.total_voxels()));

        if let Some((name, placed, voxels)) = summary {
            ui.label(name);
            ui.label(format!("{placed} placed models"));
            ui.label(format!("{voxels} voxels"));
            ui.label(format!("{} triangles", self.scene_renderer.triangle_count()));
        }

        ui.separator();
        self.palette_grid(ui);

        ui.separator();
        ui.heading("Brush");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.erase, false, "Add");
            ui.selectable_value(&mut self.erase, true, "Erase");
        });
        ui.add(egui::Slider::new(&mut self.brush_color, 1..=255).text("colour"));

        // Show the colour the brush will actually paint.
        if let Some(loaded) = &self.scene {
            let c = loaded.palette.color(self.brush_color);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 16.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect, 2.0, egui::Color32::from_rgb(c.r, c.g, c.b));
        }

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

        if ui
            .add_enabled(self.scene.is_some(), egui::Button::new("Save (Cmd+S)"))
            .clicked()
        {
            let message = self.save_command("save");
            self.console_log.push(message);
        }

        ui.separator();
        ui.heading("Layers");
        let mut toggled = false;
        ui.horizontal_wrapped(|ui| {
            for (i, hidden) in self.hidden_layers.iter_mut().enumerate() {
                let mut visible = !*hidden;
                if ui.checkbox(&mut visible, format!("{i}")).changed() {
                    *hidden = !visible;
                    toggled = true;
                }
            }
        });
        if toggled {
            self.mesh_dirty = true;
        }

        ui.separator();
        self.console_ui(ui, state);

        ui.separator();
        ui.label("Drag to orbit, scroll to zoom.");
        ui.label("Click to edit, Cmd+Z to undo.");
        ui.separator();
        ui.label("Samples");

        let mut pick = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            if let Ok(entries) = std::fs::read_dir(ASSET_DIR) {
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
            self.load(state, &path);
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
            let c = loaded.palette.color(i as u8);
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
            self.run_command(&line, state);
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

    fn run_command(&mut self, line: &str, state: &egui_wgpu::RenderState) {
        if line.trim().is_empty() {
            return;
        }
        self.console_log.push(format!("> {line}"));

        if line.split_whitespace().next() == Some("save") {
            let message = self.save_command(line);
            self.console_log.push(message);
            return;
        }

        if line.split_whitespace().next() == Some("gpu") {
            let message = self.run_gpu_command(line, state);
            self.console_log.push(message);
            return;
        }

        // Same disjoint-borrow dance as undo/redo: reach the fields directly.
        let mut changed = false;
        let mut message = None;
        if let Some(loaded) = self.scene.as_mut() {
            if let Some(index) = loaded.scene.instances.first().map(|i| i.model_index) {
                if let Some(volume) = loaded.scene.models.get_mut(index) {
                    match console::execute(line, volume, &mut self.history) {
                        console::Outcome::Ok { message: m, changed: c } => {
                            changed = c;
                            if !m.is_empty() {
                                message = Some(m);
                            }
                        }
                        console::Outcome::Error(e) => message = Some(format!("error: {e}")),
                    }
                }
            }
        }
        if let Some(m) = message {
            self.console_log.push(m);
        }
        if changed {
            self.remesh(state);
        }
    }

    /// `save [path]` — defaults to the file that was opened.
    fn save_command(&mut self, line: &str) -> String {
        let Some(loaded) = &self.scene else {
            return "nothing to save".to_owned();
        };
        let target = line
            .split_whitespace()
            .nth(1)
            .map(str::to_owned)
            .unwrap_or_else(|| loaded.path.clone());

        match loaded.save(&target) {
            Ok(bytes) => format!("saved {target} ({bytes} bytes)"),
            Err(e) => format!("error: could not save: {e}"),
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

        let Some(loaded) = self.scene.as_mut() else {
            return "no model loaded".to_owned();
        };
        let Some(index) = loaded.scene.instances.first().map(|i| i.model_index) else {
            return "no model loaded".to_owned();
        };
        let Some(volume) = loaded.scene.models.get_mut(index) else {
            return "no model loaded".to_owned();
        };

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
        self.history.set_voxels(volume, writes);
        self.history.end_gesture();
        self.mesh_dirty = true;

        format!("gpu {shape_name}: {count} voxels in {elapsed:.1?}")
    }

    fn viewport(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());

        let ppp_early = ui.ctx().pixels_per_point();
        let view_size = egui::vec2(rect.width(), rect.height());
        let _ = ppp_early;

        // A click edits; a drag orbits. egui separates the two, so the camera
        // never lurches when placing a voxel.
        if response.clicked() {
            if let Some(pos) = response.interact_pointer_pos() {
                let aspect = view_size.x / view_size.y.max(1.0);
                let vp = self.camera.view_proj(aspect);
                let local = pos - rect.min;
                self.edit_at(local, view_size, vp);
            }
        }

        // Orbit with a drag; the factor turns pixels into a comfortable
        // radians-per-pixel rate.
        if response.dragged() {
            let d = response.drag_delta();
            self.camera.orbit(d.x * 0.01, d.y * 0.01);
        }
        if response.drag_stopped() || response.clicked() {
            self.history.end_gesture();
        }

        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.camera.zoom(1.0 - scroll * 0.002);
            }
        }

        // Render at physical pixels, not logical points, or the image is soft
        // on a Retina display.
        let ppp = ui.ctx().pixels_per_point();
        let size = [
            ((rect.width() * ppp) as u32).max(1),
            ((rect.height() * ppp) as u32).max(1),
        ];
        let view_proj = self
            .camera
            .view_proj(size[0] as f32 / size[1] as f32);

        if self.mesh_dirty {
            self.remesh(state);
        }

        let mut egui_renderer = state.renderer.write();
        let texture_id = self.scene_renderer.render(
            &state.device,
            &state.queue,
            &mut egui_renderer,
            size,
            view_proj,
            self.light_dir,
            Vec3::new(0.19, 0.18, 0.26), // the editor's own default ambient
            [0.16, 0.16, 0.17],
        );
        drop(egui_renderer);

        ui.painter().image(
            texture_id,
            rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }
}

/// Layer -1 means "no layer assigned", which is always visible.
fn is_hidden(hidden: &[bool; 8], layer: i32) -> bool {
    usize::try_from(layer).ok().and_then(|i| hidden.get(i)).copied().unwrap_or(false)
}

impl eframe::App for App {
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

        let save = ui.input(|i| i.key_pressed(egui::Key::S) && i.modifiers.command);
        if save {
            let message = self.save_command("save");
            self.console_log.push(message);
        }

        egui::Panel::left("controls")
            .default_size(240.0)
            .show(ui, |ui| self.controls(ui, &state));

        egui::CentralPanel::default().show(ui, |ui| self.viewport(ui, &state));

        // Reads back the 3D target. egui's own ViewportCommand::Screenshot
        // would also capture the panels, but this eframe/wgpu backend never
        // delivers the resulting event here, so the viewport is what we can
        // verify automatically.
        if let Some(path) = self.screenshot.take() {
            match self.scene_renderer.read_frame(&state.device, &state.queue) {
                Some((w, h, rgba)) => match image_out::write_bmp(&path, w, h, &rgba) {
                    Ok(()) => eprintln!("[voxferro] wrote {path} ({w}x{h})"),
                    Err(e) => eprintln!("[voxferro] could not write {path}: {e}"),
                },
                None => eprintln!("[voxferro] nothing rendered yet; no screenshot"),
            }
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}