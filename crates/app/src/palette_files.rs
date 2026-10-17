//! Palette files: the reference editor's are PNG images, 256 × 1, whose pixel `k`
//! is palette index `k + 1` -- the order of the `RGBA` chunk -- so the last
//! pixel is the unused index 0. Any image is accepted on reading: its first
//! 256 pixels, row by row.
//!
//! - `palette open <file.png>`, `palette save <file.png>`;
//! - `palette reset`: the format specification's default palette;
//! - `palette default <0-3>`: the reference editor's four default palettes
//!   (`palette/pal0.png`...), from a local install -- they are its files,
//!   not shipped here, like its sample models.
//!
//! Replacing the palette is one undo step. Voxels keep their indices, so
//! they take the new colours.

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

/// Encodes a palette as the reference editor does.
pub fn write_png(colours: &[Color; 256]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(256 * 4);
    for k in 0..256 {
        let c = colours[(k + 1) % 256];
        rgba.extend_from_slice(&[c.r, c.g, c.b, 255]);
    }
    voxel_core::export::png_rgba(256, 1, &rgba)
}

/// the reference editor's default palette `n`, from the first install that has it.
fn reference_editor_palette(n: u8) -> Option<std::path::PathBuf> {
    crate::reference_editor_dirs()
        .into_iter()
        .map(|d| d.join("palette").join(format!("pal{n}.png")))
        .find(|p| p.is_file())
}

impl App {
    /// `palette open|save|reset|default …`; `None` if the line is the
    /// per-colour form.
    pub fn palette_file_command(&mut self, line: &str, state: &egui_wgpu::RenderState) -> Option<String> {
        let words: Vec<&str> = line.split_whitespace().collect();
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
            ["default", n] => match n.parse::<u8>().ok().filter(|n| *n <= 3) {
                Some(n) => match reference_editor_palette(n) {
                    Some(path) => {
                        return self.palette_file_command(&format!("palette open {}", path.display()), state);
                    }
                    None => format!("palette {n}: no the reference editor install with palette/pal{n}.png found"),
                },
                None => "palette default 0|1|2|3".to_owned(),
            },
            _ => return None,
        })
    }

    /// The buttons under the palette: open, save, reset, and the reference editor's
    /// four defaults when an install has them.
    pub fn palette_files_ui(&mut self, ui: &mut eframe::egui::Ui, state: &egui_wgpu::RenderState) {
        let mut line = None;
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("Open…").on_hover_text("A palette image (PNG)").clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Palette", &["png"]).pick_file()
            {
                line = Some(format!("palette open {}", path.display()));
            }
            if ui.small_button("Save…").on_hover_text("As a 256 × 1 PNG, as the reference editor saves it").clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("Palette", &["png"]).set_file_name("palette.png").save_file()
            {
                line = Some(format!("palette save {}", path.display()));
            }
            if ui.small_button("Reset").on_hover_text("The format's default palette").clicked() {
                line = Some("palette reset".to_owned());
            }
            for n in 0..4u8 {
                if reference_editor_palette(n).is_some()
                    && ui.small_button(n.to_string()).on_hover_text(format!("the reference editor's default palette {n}")).clicked()
                {
                    line = Some(format!("palette default {n}"));
                }
            }
        });
        if let Some(line) = line {
            let message = self.palette_file_command(&line, state).unwrap_or_default();
            self.console_log.push(message);
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
    fn a_palette_survives_the_round_trip_in_reference_editor_order() {
        let mut colours = *vox_format::Palette::specification_default().as_slice();
        colours[255] = Color { r: 1, g: 2, b: 3, a: 255 };
        let png = write_png(&colours);
        let back = read_png(&png).unwrap();
        assert_eq!(&back[1..], &colours[1..]);
        // Pixel 0 is index 1.
        let first = read_png(&voxel_core::export::png_rgba(1, 1, &[9, 8, 7, 255])).unwrap();
        assert_eq!(first[1], Color { r: 9, g: 8, b: 7, a: 255 });
    }

    /// the reference editor's own palettes read, when an install is around.
    #[test]
    fn reference_editor_palettes_read() {
        if let Some(path) = reference_editor_palette(0) {
            let colours = read_png(&std::fs::read(path).unwrap()).unwrap();
            // pal0 starts with white.
            assert_eq!(colours[1], Color { r: 255, g: 255, b: 255, a: 255 });
        }
    }
}
