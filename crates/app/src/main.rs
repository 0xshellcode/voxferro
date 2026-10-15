//! Entry point for the voxel editor.
//!
//! eframe owns the window and the event loop; we draw the scene into it with
//! wgpu through an egui paint callback (phase 2). On Apple Silicon wgpu emits
//! Metal directly, so nothing here goes through the deprecated OpenGL stack.

use eframe::egui;

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

struct App {
    /// What wgpu actually bound to, reported by the adapter itself.
    ///
    /// Phase 0 shows this so the backend is a fact on screen rather than an
    /// assumption: on this machine it should read Metal.
    backend: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let backend = match cc.wgpu_render_state.as_ref() {
            Some(state) => {
                let info = state.adapter.get_info();
                format!("{:?} on {}", info.backend, info.name)
            }
            None => "no wgpu render state (eframe fell back to glow?)".to_owned(),
        };
        Self { backend }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Voxferro");
            ui.label("Phase 0: scaffolding in place.");
            ui.separator();
            ui.label(format!("Graphics backend: {}", self.backend));
            ui.label("Next: phase 1, the .vox parser.");
        });
    }
}
