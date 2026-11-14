//! Palette files: PNG images, 256 × 1, whose pixel `k`
//! is palette index `k + 1` -- the order of the `RGBA` chunk -- so the last
//! pixel is the unused index 0. Any image is accepted on reading: its first
//! 256 pixels, row by row.
//!
//! - `palette open <file.png>`, `palette save <file.png>`;
//! - `palette reset`: the format specification's default palette.
//!
//! And the palette's own editing:
//!
//! - `palette swap <a> <b>`: two colours change places, with their
//!   materials (Ctrl+drag);
//! - `palette gradient <a> <b>`: the colours between two indices blend from
//!   one to the other (Ctrl+Alt+drag);
//! - `palette copy <a> <b>`, `palette paste <i>`: a run of colours copied
//!   and written again from another index (Copy and Paste Palette Region).
//!
//! Each is one undo step. Voxels keep their indices, so they take the new
//! colours.

use crate::App;
use eframe::egui_wgpu;
use vox_format::Color;
use voxel_core::Command;

/// Decodes a palette image.
pub fn read_png(bytes: &[u8]) -> Result<[Color; 256], String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let size = reader.output_buffer_size().ok_or("the image is too large")?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => return Err("unexpanded indexed image".to_owned()),
    };
    let pixels = (info.width * info.height) as usize;
    let mut colours = [Color::default(); 256];
    for k in 0..pixels.min(256) {
        let p = &buf[k * channels..(k + 1) * channels];
        let (r, g, b) = if channels >= 3 { (p[0], p[1], p[2]) } else { (p[0], p[0], p[0]) };
        colours[(k + 1) % 256] = Color { r, g, b, a: 255 };
    }
    Ok(colours)
}

/// Any PNG as RGBA, for textures.
pub fn read_rgba(bytes: &[u8]) -> Option<voxel_core::voxelize::Texture> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => return None,
    };
    let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
    for p in buf[..(info.width * info.height) as usize * channels].chunks(channels) {
        let (r, g, b) = if channels >= 3 { (p[0], p[1], p[2]) } else { (p[0], p[0], p[0]) };
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    (info.width > 0 && info.height > 0).then_some(voxel_core::voxelize::Texture { width: info.width, height: info.height, rgba })
}

/// Encodes a palette as a 256 × 1 PNG.
pub fn write_png(colours: &[Color; 256]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(256 * 4);
    for k in 0..256 {
        let c = colours[(k + 1) % 256];
        rgba.extend_from_slice(&[c.r, c.g, c.b, 255]);
    }
    voxel_core::export::png_rgba(256, 1, &rgba)
}

/// A palette index for editing: 1 to 255.
fn index(word: &str) -> Option<u8> {
    word.parse::<u8>().ok().filter(|&i| i > 0)
}

impl App {
    /// `palette open|save|reset|default …`; `None` if the line is the
    /// per-colour form.
    pub fn palette_file_command(&mut self, line: &str, state: &egui_wgpu::RenderState) -> Option<String> {
        let words: Vec<&str> = line.split_whitespace().collect();
        // Only `palette ...`: `material reset` once reset the whole palette
        // through here.
        if words.first() != Some(&"palette") {
            return None;
        }
        Some(match words.get(1..).unwrap_or_default() {
            ["open", path @ ..] if !path.is_empty() => {
                let path = path.join(" ");
                match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| read_png(&b)) {
                    Ok(colours) => self.replace_palette(&colours, &format!("palette from {path}"), state),
                    Err(e) => format!("error: {path}: {e}"),
                }
            }
            ["save", path @ ..] if !path.is_empty() => {
                let path = path.join(" ");
                let Some(loaded) = &self.scene else { return Some("no document".to_owned()) };
                match std::fs::write(&path, write_png(loaded.doc.palette.as_slice())) {
                    Ok(()) => format!("saved the palette to {path}"),
                    Err(e) => format!("error: could not write {path}: {e}"),
                }
            }
            ["reset"] => {
                let colours = *vox_format::Palette::specification_default().as_slice();
                self.replace_palette(&colours, "palette reset to the format's default", state)
            }
            ["swap", a, b] => match (index(a), index(b)) {
                (Some(a), Some(b)) => {
                    let Some(loaded) = self.scene.as_ref() else { return Some("no document".to_owned()) };
                    let doc = &loaded.doc;
                    let (ca, cb) = (doc.palette.color(a), doc.palette.color(b));
                    let (ma, mb) = (doc.material(a).cloned(), doc.material(b).cloned());
                    // A material names its index: moved, it is renumbered.
                    let renumber = |m: Option<vox_format::Material>, i: u8| m.map(|mut m| { m.id = i as i32; m });
                    let commands = vec![
                        Command::SetColor { index: a, before: ca, after: cb },
                        Command::SetColor { index: b, before: cb, after: ca },
                        Command::SetMaterial { index: a, before: ma.clone(), after: renumber(mb.clone(), a) },
                        Command::SetMaterial { index: b, before: mb, after: renumber(ma, b) },
                    ];
                    self.palette_batch(commands, &format!("swapped colours {a} and {b}"), state)
                }
                _ => "palette swap <1-255> <1-255>".to_owned(),
            },
            ["gradient", a, b] => match (index(a), index(b)) {
                (Some(a), Some(b)) if a.abs_diff(b) >= 2 => {
                    let (lo, hi) = (a.min(b), a.max(b));
                    let Some(loaded) = self.scene.as_ref() else { return Some("no document".to_owned()) };
                    let (from, to) = (loaded.doc.palette.color(lo), loaded.doc.palette.color(hi));
                    let span = (hi - lo) as f32;
                    let mix = |p: u8, q: u8, t: f32| (p as f32 + (q as f32 - p as f32) * t).round() as u8;
                    let commands = (lo + 1..hi)
                        .map(|i| {
                            let t = (i - lo) as f32 / span;
                            let after = Color { r: mix(from.r, to.r, t), g: mix(from.g, to.g, t), b: mix(from.b, to.b, t), a: 255 };
                            Command::SetColor { index: i, before: loaded.doc.palette.color(i), after }
                        })
                        .collect();
                    self.palette_batch(commands, &format!("gradient from {lo} to {hi}"), state)
                }
                _ => "palette gradient <a> <b>, at least one colour between them".to_owned(),
            },
            ["copy", a, b] => match (index(a), index(b)) {
                (Some(a), Some(b)) => {
                    let Some(loaded) = self.scene.as_ref() else { return Some("no document".to_owned()) };
                    let (lo, hi) = (a.min(b), a.max(b));
                    self.palette_clip = (lo..=hi).map(|i| loaded.doc.palette.color(i)).collect();
                    format!("copied colours {lo} to {hi}")
                }
                _ => "palette copy <a> <b>".to_owned(),
            },
            ["paste", at] => match index(at) {
                Some(at) if !self.palette_clip.is_empty() => {
                    let Some(loaded) = self.scene.as_ref() else { return Some("no document".to_owned()) };
                    let commands = self
                        .palette_clip
                        .iter()
                        .zip(at..=255)
                        .map(|(c, i)| Command::SetColor { index: i, before: loaded.doc.palette.color(i), after: *c })
                        .collect();
                    self.palette_batch(commands, &format!("pasted {} colours at {at}", self.palette_clip.len()), state)
                }
                Some(_) => "nothing copied: palette copy <a> <b> first".to_owned(),
                None => "palette paste <1-255>".to_owned(),
            },
            _ => return None,
        })
    }

    pub(crate) fn palette_batch(&mut self, commands: Vec<Command>, what: &str, state: &egui_wgpu::RenderState) -> String {
        let Some(loaded) = self.scene.as_mut() else { return "no document".to_owned() };
        self.history.end_gesture();
        if self.history.exec_alone(&mut loaded.doc, Command::Batch(commands)) {
            self.refresh_colors(state);
            // Materials decide what the tracer samples as light.
            self.upload_trace_volume(state);
            what.to_owned()
        } else {
            format!("{what}: nothing changed")
        }
    }

    /// Every colour but index 0 replaced, as one undo step.
    fn replace_palette(&mut self, colours: &[Color; 256], what: &str, state: &egui_wgpu::RenderState) -> String {
        let Some(loaded) = self.scene.as_mut() else { return "no document".to_owned() };
        let commands: Vec<Command> = (1..=255u8)
            .filter_map(|index| {
                let before = loaded.doc.palette.color(index);
                let after = colours[index as usize];
                (before != after).then_some(Command::SetColor { index, before, after })
            })
            .collect();
        let changed = commands.len();
        self.history.end_gesture();
        if changed > 0 && self.history.exec_alone(&mut loaded.doc, Command::Batch(commands)) {
            self.refresh_colors(state);
        }
        format!("{what}: {changed} colours changed")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_palette_survives_the_round_trip_in_rgba_order() {
        let mut colours = *vox_format::Palette::specification_default().as_slice();
        colours[255] = Color { r: 1, g: 2, b: 3, a: 255 };
        let png = write_png(&colours);
        let back = read_png(&png).unwrap();
        assert_eq!(&back[1..], &colours[1..]);
        // Pixel 0 is index 1.
        let first = read_png(&voxel_core::export::png_rgba(1, 1, &[9, 8, 7, 255])).unwrap();
        assert_eq!(first[1], Color { r: 9, g: 8, b: 7, a: 255 });
    }
}
