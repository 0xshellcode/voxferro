//! The final render at a size of its own: Render › Image.
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
//!
//! **Turntable**: `render spin <frames> <degrees>
//! <blur %> <width> <height> <samples> <file.png>` renders a photo a frame,
//! the camera turned about the point it orbits by degrees / frames each
//! time, from where it is -- 360 makes a loop. With blur, each sample of a
//! frame turns a random amount within that share of its step, centred on
//! the frame: motion blur. The frames are written as `file (0001).png`,
//! `file (0002).png`..., and the camera goes
//! back where it was.

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
    /// A turntable's frames, when this is one.
    spin: Option<Spin>,
}

/// A turntable under way.
#[derive(Clone, Copy)]
pub struct Spin {
    frames: u32,
    /// The camera's turn per frame, radians.
    step: f32,
    /// The share of a step each frame's samples spread over.
    blur: f32,
    /// Where the camera started, and the frame being rendered.
    start_yaw: f32,
    frame: u32,
}

/// `file.png` as a turntable's frame `k` (from 0): `file (0001).png`.
pub fn frame_path(path: &str, k: u32) -> String {
    let stem = path.strip_suffix(".png").unwrap_or(path);
    format!("{stem} ({:04}).png", k + 1)
}

/// The panel's settings, kept between photos.
pub struct PhotoSettings {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    /// The turntable's frames, how far round, and its blur in percent.
    pub frames: u32,
    pub degrees: f32,
    pub blur: f32,
}

impl Default for PhotoSettings {
    fn default() -> Self {
        Self { width: 1920, height: 1080, samples: 256, frames: 36, degrees: 360.0, blur: 0.0 }
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
        self.photo = Some(PhotoJob { size, samples, path, started: Instant::now(), quit_when_done, spin: None });
        format!("rendering {}x{} to {samples} samples", size[0], size[1])
    }

    /// Starts a turntable: `frames` photos, `degrees` round in all.
    pub fn start_spin(&mut self, frames: u32, degrees: f32, blur: f32, size: [u32; 2], samples: u32, path: String) -> String {
        if frames == 0 || frames > 10_000 {
            return "render spin: 1 to 10000 frames".to_owned();
        }
        let message = self.start_photo(size, samples, path);
        if let Some(job) = &mut self.photo {
            let step = (degrees / frames as f32).to_radians();
            job.spin = Some(Spin { frames, step, blur: (blur / 100.0).clamp(0.0, 1.0), start_yaw: self.camera.yaw, frame: 0 });
            return format!("turntable: {frames} frames, {degrees}° round; {message}");
        }
        message
    }

    /// Before each traced sample of a turntable: the camera at its frame,
    /// turned by a random part of the blur.
    pub fn spin_camera(&mut self) {
        let Some(Some(spin)) = self.photo.as_ref().map(|j| j.spin) else { return };
        let jitter = if spin.blur > 0.0 { (self.spin_rand.next_u32() as f32 / u32::MAX as f32 - 0.5) * spin.blur } else { 0.0 };
        self.camera.yaw = spin.start_yaw + spin.step * (spin.frame as f32 + jitter);
    }

    /// `render image <w> <h> <samples> <file.png>`, `render image cancel`.
    pub fn photo_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        match words.as_slice() {
            ["image" | "spin", "cancel"] => match self.photo.take() {
                Some(_) => {
                    self.path_tracer.reset();
                    "render image cancelled".to_owned()
                }
                None => "no render image running".to_owned(),
            },
            ["spin", n, deg, blur, w, h, s, path @ ..] if !path.is_empty() => {
                match (n.parse(), deg.parse(), blur.parse(), w.parse(), h.parse(), s.parse()) {
                    (Ok(n), Ok(deg), Ok(blur), Ok(w), Ok(h), Ok(s)) => self.start_spin(n, deg, blur, [w, h], s, path.join(" ")),
                    _ => "render spin <frames> <degrees> <blur %> <width> <height> <samples> <file.png>".to_owned(),
                }
            }
            ["image", w, h, s, path @ ..] if !path.is_empty() => match (w.parse(), h.parse(), s.parse()) {
                (Ok(w), Ok(h), Ok(s)) => self.start_photo([w, h], s, path.join(" ")),
                _ => "render image <width> <height> <samples> <file.png> | render spin <frames> <degrees> <blur %> <width> <height> <samples> <file.png> | render image cancel".to_owned(),
            },
            _ => "render image <width> <height> <samples> <file.png> | render spin <frames> <degrees> <blur %> <width> <height> <samples> <file.png> | render image cancel".to_owned(),
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
        let path = match job.spin {
            Some(spin) => frame_path(&job.path, spin.frame),
            None => job.path.clone(),
        };
        let message = match self.path_tracer.read_frame(&state.device, &state.queue) {
            Some((w, h, rgba)) => match std::fs::write(&path, voxel_core::export::png_rgba(w, h, &rgba)) {
                Ok(()) => format!("saved {path} ({w}x{h}, {} samples, {:.1?})", self.path_tracer.samples, job.started.elapsed()),
                Err(e) => format!("error: could not write {path}: {e}"),
            },
            None => "error: the render could not be read back".to_owned(),
        };
        eprintln!("[voxferro] {message}");
        let failed = message.starts_with("error");
        self.console_log.push(message);
        self.path_tracer.reset();
        // A turntable goes on to its next frame, or puts the camera back.
        if let Some(spin) = job.spin {
            if spin.frame + 1 < spin.frames && !failed {
                self.photo = Some(PhotoJob { spin: Some(Spin { frame: spin.frame + 1, ..spin }), ..job });
                ctx.request_repaint();
                return;
            }
            self.camera.yaw = spin.start_yaw;
        }
        if job.quit_when_done {
            self.allow_close = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The progress, over the letterboxed image.
    pub fn photo_overlay(&self, ui: &egui::Ui, rect: egui::Rect) {
        let Some(job) = &self.photo else { return };
        let frame = job.spin.map_or(String::new(), |s| format!("frame {} / {}  ·  ", s.frame + 1, s.frames));
        let text = format!(
            "{frame}{}×{}  ·  {} / {} samples  ·  {:.0?}",
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
            ui.label("Frames").on_hover_text("Turntable: how many frames");
            ui.add(egui::DragValue::new(&mut s.frames).range(1..=10000));
            ui.end_row();
            ui.label("Degrees").on_hover_text("Turntable: how far round in all; 360 loops");
            ui.add(egui::DragValue::new(&mut s.degrees).range(-3600.0..=3600.0));
            ui.end_row();
            ui.label("Blur %").on_hover_text("Turntable: motion blur, as a share of each frame's turn");
            ui.add(egui::DragValue::new(&mut s.blur).range(0.0..=100.0));
            ui.end_row();
        });
        if ui.button("Turntable…").on_hover_text("Render the frames of a turn round the model, as numbered PNGs").clicked() {
            let dialog = rfd::FileDialog::new().add_filter("PNG image", &["png"]).set_file_name("spin.png");
            if let Some(path) = dialog.save_file() {
                let s = &self.photo_settings;
                let (frames, degrees, blur, size, samples) = (s.frames, s.degrees, s.blur, [s.width, s.height], s.samples);
                let message = self.start_spin(frames, degrees, blur, size, samples, path.to_string_lossy().into_owned());
                self.console_log.push(message);
            }
        }
        if ui.button("Render image…").on_hover_text("Render at this size to this many samples, then save as PNG").clicked() {
            let stem = self.scene.as_ref().and_then(|s| std::path::Path::new(&s.name).file_stem().map(|n| n.to_string_lossy().into_owned()));
            let dialog = rfd::FileDialog::new()
                .add_filter("PNG image", &["png"])
                .set_file_name(format!("{}.png", stem.unwrap_or_else(|| "render".to_owned())));
            if let Some(path) = dialog.save_file() {
                let PhotoSettings { width, height, samples, .. } = self.photo_settings;
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
    fn turntable_frames_are_numbered_from_one() {
        assert_eq!(frame_path("/tmp/spin.png", 0), "/tmp/spin (0001).png");
        assert_eq!(frame_path("out", 41), "out (0042).png");
    }

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
