//! Entry point for the voxel editor.
//!
//! eframe owns the window and the event loop. The scene is rendered with wgpu
//! into an offscreen texture that egui then displays, which is what gives us a
//! depth buffer (see `renderer`). On Apple Silicon wgpu emits Metal directly,
//! so nothing goes through the deprecated OpenGL stack.

mod camera;
mod image_out;
mod renderer;
mod scene;

use camera::OrbitCamera;
use eframe::{egui, egui_wgpu};
use glam::Vec3;
use renderer::SceneRenderer;
use voxel_core::Mesh;

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
    status: String,
    /// Index into the scene's model list.
    shown_model: usize,
    light_dir: Vec3,
    /// When set, the app writes one rendered frame here and exits. Used to
    /// verify the viewport without a human looking at the window, and to
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
            status: String::new(),
            shown_model: 0,
            light_dir: Vec3::new(0.3, 0.5, 0.8).normalize(),
            screenshot: None,
        };

        // Usage: voxferro [model.vox] [--screenshot out.bmp]
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

        app.screenshot = screenshot;
        app.load(state, &path);
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

        let mesh = Mesh::from_scene(&loaded.scene);
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
        ui.label("Drag to orbit, scroll to zoom.");
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

    fn viewport(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), egui::Sense::drag());

        // Orbit with a drag; the factor turns pixels into a comfortable
        // radians-per-pixel rate.
        if response.dragged() {
            let d = response.drag_delta();
            self.camera.orbit(d.x * 0.01, d.y * 0.01);
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
        egui::Panel::left("controls")
            .default_size(240.0)
            .show(ui, |ui| self.controls(ui, &state));

        egui::CentralPanel::default().show(ui, |ui| self.viewport(ui, &state));

        // One frame is enough: the viewport above has already rendered.
        if let Some(path) = self.screenshot.take() {
            match self.scene_renderer.read_frame(&state.device, &state.queue) {
                Some((w, h, rgba)) => match image_out::write_bmp(&path, w, h, &rgba) {
                    Ok(()) => eprintln!("[voxferro] wrote {path} ({w}x{h})"),
                    Err(e) => eprintln!("[voxferro] could not write {path}: {e}"),
                },
                None => eprintln!("[voxferro] nothing rendered yet; no screenshot"),
            }
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
