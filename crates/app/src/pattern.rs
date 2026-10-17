//! The pattern: a small model the pattern brush stamps and the Edit panel's
//! Mask and Texture tile.
//!
//! `pattern` reports it; `pattern selection` makes it from the selected
//! voxels (the reference editor's "Create Pattern from Selection"); `pattern open
//! <file.vox>` takes the first model of a file; `pattern turn x|y|z` gives it
//! a quarter turn; `pattern colours own|brush` chooses between its own
//! colours and the current one ("Use Voxel Color" / "Use Palette Color");
//! `pattern clear` drops it. A pattern's colours are palette indices; one
//! opened from another file has each index moved to the nearest colour of
//! this palette, unless `pattern match off` (the reference editor's "Match Palette
//! Color", on by default there too) keeps the indices as they are.

use crate::App;
use eframe::egui;
use voxel_core::{Volume, ops};

impl App {
    pub fn pattern_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.get(1..).unwrap_or_default() {
            [] => self.pattern_info(),
            ["selection"] => match self.pattern_from_selection() {
                Some(p) => {
                    self.pattern = Some(p);
                    self.pattern_info()
                }
                None => "pattern: nothing selected".to_owned(),
            },
            ["open", path @ ..] if !path.is_empty() => {
                let path = path.join(" ");
                match load_pattern(&path) {
                    Ok((volume, palette)) => {
                        let volume = match (&self.scene, self.pattern_match) {
                            (Some(loaded), true) => match_colours(&volume, &palette, &loaded.doc.palette),
                            _ => volume,
                        };
                        self.pattern = Some(volume);
                        self.pattern_info()
                    }
                    Err(e) => format!("pattern: {e}"),
                }
            }
            ["match", on] => {
                match *on {
                    "on" => self.pattern_match = true,
                    "off" => self.pattern_match = false,
                    _ => return "pattern match on|off".to_owned(),
                }
                format!("patterns opened from now on {} their colours", if self.pattern_match { "match" } else { "keep" })
            }
            ["turn", axis] => {
                let a = match *axis {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    _ => return "pattern turn x|y|z".to_owned(),
                };
                match &self.pattern {
                    Some(p) => {
                        self.pattern = Some(ops::rotate(p, a));
                        self.pattern_info()
                    }
                    None => "pattern: none yet".to_owned(),
                }
            }
            ["colours" | "colors", which] => {
                match *which {
                    "own" => self.pattern_own_colours = true,
                    "brush" => self.pattern_own_colours = false,
                    _ => return "pattern colours own|brush".to_owned(),
                }
                self.pattern_info()
            }
            ["clear"] => {
                self.pattern = None;
                "pattern cleared".to_owned()
            }
            _ => "pattern [selection | open <file.vox> | match on|off | turn x|y|z | colours own|brush | clear]".to_owned(),
        }
    }

    fn pattern_info(&self) -> String {
        let colours = if self.pattern_own_colours { "its own colours" } else { "the brush colour" };
        match &self.pattern {
            Some(p) => {
                let [x, y, z] = p.size();
                format!("pattern {x}x{y}x{z}, {} voxels, in {colours}", p.filled_count())
            }
            None => "no pattern".to_owned(),
        }
    }

    /// The selected voxels, in the smallest box that holds them.
    fn pattern_from_selection(&self) -> Option<Volume> {
        let selection = self.active_selection()?;
        let volume = self.scene.as_ref()?.doc.scene.models.get(selection.model)?;
        let mut cells = selection.iter().filter(|p| volume.get_i(p.x, p.y, p.z) != 0);
        let first = cells.next()?;
        let (lo, hi) = cells.fold((first, first), |(a, b), c| (a.min(c), b.max(c)));
        let size = (hi - lo + glam::IVec3::ONE).to_array().map(|s| s as u32);
        let mut out = Volume::new(size);
        for p in selection.iter() {
            let c = volume.get_i(p.x, p.y, p.z);
            if c != 0 {
                let q = p - lo;
                out.set(q.x as u32, q.y as u32, q.z as u32, c);
            }
        }
        Some(out)
    }

    /// The pattern brush's click: the pattern placed on the hit face, then
    /// attached, erased or painted like any stroke.
    pub fn pattern_stroke_writes(&self, volume: &Volume, hit: &voxel_core::Hit) -> Option<Vec<(glam::IVec3, u8)>> {
        let pattern = self.pattern.as_ref()?;
        let colour = (!self.pattern_own_colours).then_some(self.brush_color);
        let cells = voxel_core::brush::pattern_cells(pattern, self.brush_mode, hit, colour);
        Some(voxel_core::brush::pattern_writes(volume, self.brush_mode, &cells))
    }

    pub fn pattern_ui(&mut self, ui: &mut egui::Ui) {
        let mut lines = Vec::new();
        ui.label(self.pattern_info());
        ui.horizontal_wrapped(|ui| {
            if ui.small_button("From selection").on_hover_text("Make the pattern from the selected voxels").clicked() {
                lines.push("pattern selection".to_owned());
            }
            if ui.small_button("Open…").on_hover_text("The first model of a .vox file").clicked()
                && let Some(path) = rfd::FileDialog::new().add_filter("the reference editor", &["vox"]).pick_file()
            {
                lines.push(format!("pattern open {}", path.display()));
            }
            for a in ["x", "y", "z"] {
                if ui.small_button(format!("Turn {a}")).clicked() {
                    lines.push(format!("pattern turn {a}"));
                }
            }
        });
        ui.horizontal(|ui| {
            let mut own = self.pattern_own_colours;
            if ui.checkbox(&mut own, "Its own colours").on_hover_text("Otherwise the brush colour").changed() {
                lines.push(format!("pattern colours {}", if own { "own" } else { "brush" }));
            }
            let mut matching = self.pattern_match;
            if ui
                .checkbox(&mut matching, "Match")
                .on_hover_text("A pattern opened from another file takes the nearest colours of this palette")
                .changed()
            {
                lines.push(format!("pattern match {}", if matching { "on" } else { "off" }));
            }
        });
        // The library: the project folder's files, a click away.
        egui::CollapsingHeader::new("Library").id_salt("pattern library").show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("pattern files").max_height(140.0).show(ui, |ui| {
                for path in self.project_vox_files() {
                    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    if ui.selectable_label(false, name).on_hover_text("Use as the pattern").clicked() {
                        lines.push(format!("pattern open {}", path.display()));
                    }
                }
            });
        });
        for line in lines {
            let message = self.pattern_command(&line);
            self.console_log.push(message);
        }
    }
}

fn load_pattern(path: &str) -> Result<(Volume, vox_format::Palette), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let file = vox_format::VoxFile::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let model = file.models.first().ok_or_else(|| format!("{path}: no models"))?;
    Ok((Volume::from_model(model), file.palette))
}

/// `volume`'s colours, from palette `from`, moved to the nearest colours of
/// `to` (by distance in RGB; index 0 is never a target).
pub fn match_colours(volume: &Volume, from: &vox_format::Palette, to: &vox_format::Palette) -> Volume {
    let mut map = [0u8; 256];
    for (i, slot) in map.iter_mut().enumerate().skip(1) {
        let c = from.color(i as u8);
        let distance = |j: u8| {
            let d = to.color(j);
            let (r, g, b) = (c.r as i32 - d.r as i32, c.g as i32 - d.g as i32, c.b as i32 - d.b as i32);
            r * r + g * g + b * b
        };
        *slot = (1..=255u8).min_by_key(|&j| distance(j)).unwrap_or(1);
    }
    let [sx, sy, sz] = volume.size();
    let mut out = Volume::new([sx, sy, sz]);
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let v = volume.get(x, y, z);
                if v != 0 {
                    out.set(x, y, z, map[v as usize]);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vox_format::{Color, Palette};

    #[test]
    fn matching_moves_each_colour_to_the_nearest() {
        let mut from = Palette::default();
        from.set_color(3, Color { r: 250, g: 0, b: 0, a: 255 });
        let mut to = Palette::default();
        to.set_color(77, Color { r: 255, g: 0, b: 0, a: 255 });
        let mut v = Volume::new([2, 1, 1]);
        v.set(0, 0, 0, 3);
        let m = match_colours(&v, &from, &to);
        assert_eq!(m.get(0, 0, 0), 77);
        assert_eq!(m.get(1, 0, 0), 0);
    }
}
