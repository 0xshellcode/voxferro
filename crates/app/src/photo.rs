//! The final render at a size of its own: the reference editor's Render › Image
//! (`r_image.ui`: `pt_photo_w`, `pt_photo_h`, `pt_spp`, `task photo`).
//!
//! The path tracer usually renders at the viewport's size, which is
//! whatever the window happens to be. A photo renders at the size asked
//! for -- 1920×1080, say, or a square for an icon -- until it has the
//! samples asked for, then writes the PNG. Meanwhile the viewport shows it
//! letterboxed, with its progress; leaving Render mode, or `render image
//! cancel`, stops it.
//!
//! `render image <width> <height> <samples> <file.png>` from the console;
//! the Render column's IMAGE section from the panel. Started by `--run` or
//! `--script` at launch, the editor quits once the file is written, so
//! `voxferro scene.vox --run "render image 3840 2160 512 out.png"` renders
//! without anyone at the window.

use crate::App;
use eframe::{egui, egui_wgpu};
use std::time::Instant;

/// The largest side: a texture's limit on every backend wgpu supports.
pub const MAX_SIDE: u32 = 8192;

pub struct PhotoJob {
    pub size: [u32; 2],
    pub samples: u32,
    pub path: String,
    started: Instant,
    /// Started by a launch script: quit when done.
    quit_when_done: bool,
}

/// The panel's settings, kept between photos.
pub struct PhotoSettings {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
}

impl Default for PhotoSettings {
    fn default() -> Self {
        Self { width: 1920, height: 1080, samples: 256 }
    }
}

/// `rect` shrunk to `aspect` (width over height), centred.
pub fn letterbox(rect: egui::Rect, aspect: f32) -> egui::Rect {
    let (w, h) = (rect.width(), rect.height());
    let size = if w / h > aspect { egui::vec2(h * aspect, h) } else { egui::vec2(w, w / aspect) };
    egui::Rect::from_center_size(rect.center(), size)
}

impl App {
    pub fn start_photo(&mut self, size: [u32; 2], samples: u32, path: String) -> String {
        if size.iter().any(|&s| s == 0 || s > MAX_SIDE) || samples == 0 {
            return format!("render image: each side 1 to {MAX_SIDE}, and at least one sample");
        }
        self.rendering = true;
        self.path_tracer.reset();
        let quit_when_done = self.starting_up;
        self.photo = Some(PhotoJob { size, samples, path, started: Instant::now(), quit_when_done });
        format!("rendering {}x{} to {samples} samples", size[0], size[1])
    }

    /// `render image <w> <h> <samples> <file.png>`, `render image cancel`.
    pub fn photo_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(2).collect();
        match words.as_slice() {
            ["cancel"] => match self.photo.take() {
                Some(_) => {
                    self.path_tracer.reset();
                    "render image cancelled".to_owned()
                }
                None => "no render image running".to_owned(),
            },
            [w, h, s, path @ ..] if !path.is_empty() => match (w.parse(), h.parse(), s.parse()) {
                (Ok(w), Ok(h), Ok(s)) => self.start_photo([w, h], s, path.join(" ")),
                _ => "render image <width> <height> <samples> <file.png> | render image cancel".to_owned(),
            },
            _ => "render image <width> <height> <samples> <file.png> | render image cancel".to_owned(),
        }
    }

    /// The size to trace at: the photo's while one runs.
    pub fn photo_size(&self) -> Option<[u32; 2]> {
        self.photo.as_ref().map(|p| p.size)
    }

    /// After each traced frame: writes the photo once it has its samples.
    pub fn photo_step(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        let Some(job) = &self.photo else { return };
        if !self.rendering {
            self.photo = None;
            self.console_log.push("render image stopped: left Render mode".to_owned());
            return;
        }
        if self.path_tracer.samples < job.samples {
            ctx.request_repaint();
            return;
        }
        let job = self.photo.take().expect("checked above");
        let message = match self.path_tracer.read_frame(&state.device, &state.queue) {
            Some((w, h, rgba)) => match std::fs::write(&job.path, voxel_core::export::png_rgba(w, h, &rgba)) {
                Ok(()) => format!("saved {} ({w}x{h}, {} samples, {:.1?})", job.path, self.path_tracer.samples, job.started.elapsed()),
                Err(e) => format!("error: could not write {}: {e}", job.path),
            },
            None => "error: the render could not be read back".to_owned(),
        };
        eprintln!("[voxferro] {message}");
        self.console_log.push(message);
        // Back to the viewport's own size.
        self.path_tracer.reset();
        if job.quit_when_done {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The progress, over the letterboxed image.
    pub fn photo_overlay(&self, ui: &egui::Ui, rect: egui::Rect) {
        let Some(job) = &self.photo else { return };
        let text = format!(
            "{}×{}  ·  {} / {} samples  ·  {:.0?}",
            job.size[0],
            job.size[1],
            self.path_tracer.samples.min(job.samples),
            job.samples,
            job.started.elapsed()
        );
        let at = rect.left_bottom() + egui::vec2(8.0, -8.0);
        let galley = ui.painter().layout_no_wrap(text, egui::FontId::proportional(12.0), egui::Color32::WHITE);
        let back = egui::Rect::from_min_size(at - egui::vec2(4.0, galley.size().y + 4.0), galley.size() + egui::vec2(8.0, 6.0));
        ui.painter().rect_filled(back, 4.0, egui::Color32::from_black_alpha(150));
        ui.painter().galley(at - egui::vec2(0.0, galley.size().y + 1.0), galley, egui::Color32::WHITE);
    }

    /// The Render column's IMAGE section.
    pub fn photo_ui(&mut self, ui: &mut egui::Ui) {
        crate::layout::caption(ui, "IMAGE");
        if let Some(job) = &self.photo {
            let done = self.path_tracer.samples.min(job.samples) as f32 / job.samples as f32;
            ui.add(egui::ProgressBar::new(done).text(format!("{}×{}", job.size[0], job.size[1])));
            if ui.button("Cancel").clicked() {
                let message = self.photo_command("render image cancel");
                self.console_log.push(message);
            }
            return;
        }
        let s = &mut self.photo_settings;
        egui::Grid::new("photo size").num_columns(2).show(ui, |ui| {
            ui.label("Width");
            ui.add(egui::DragValue::new(&mut s.width).range(1..=MAX_SIDE));
            ui.end_row();
            ui.label("Height");
            ui.add(egui::DragValue::new(&mut s.height).range(1..=MAX_SIDE));
            ui.end_row();
            ui.label("Samples");
            ui.add(egui::DragValue::new(&mut s.samples).range(1..=65536));
            ui.end_row();
        });
        if ui.button("Render image…").on_hover_text("Render at this size to this many samples, then save as PNG").clicked() {
            let stem = self.scene.as_ref().and_then(|s| std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()));
            let dialog = rfd::FileDialog::new()
                .add_filter("PNG image", &["png"])
                .set_file_name(format!("{}.png", stem.unwrap_or_else(|| "render".to_owned())));
            if let Some(path) = dialog.save_file() {
                let PhotoSettings { width, height, samples } = self.photo_settings;
                let message = self.start_photo([width, height], samples, path.to_string_lossy().into_owned());
                self.console_log.push(message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterboxing_keeps_the_aspect_and_the_centre() {
        let r = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 400.0));
        let wide = letterbox(r, 16.0 / 9.0);
        assert!((wide.width() / wide.height() - 16.0 / 9.0).abs() < 1e-4);
        assert_eq!(wide.height(), 400.0);
        let tall = letterbox(r, 0.5);
        assert_eq!((tall.width(), tall.height()), (200.0, 400.0));
        assert_eq!(tall.center(), r.center());
    }
}
