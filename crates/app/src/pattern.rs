//! The pattern: a small model the pattern brush stamps and the Edit panel's
//! Mask and Texture tile.
//!
//! `pattern` reports it; `pattern selection` makes it from the selected
//! voxels (the reference editor's "Create Pattern from Selection"); `pattern open
//! <file.vox>` takes the first model of a file; `pattern turn x|y|z` gives it
//! a quarter turn; `pattern colours own|brush` chooses between its own
//! colours and the current one ("Use Voxel Color" / "Use Palette Color");
//! `pattern clear` drops it. A pattern's colours are palette indices, so
//! one from another file takes this file's colours at those indices.

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
                    Ok(p) => {
                        self.pattern = Some(p);
                        self.pattern_info()
                    }
                    Err(e) => format!("pattern: {e}"),
                }
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
            _ => "pattern [selection | open <file.vox> | turn x|y|z | colours own|brush | clear]".to_owned(),
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
        let mut own = self.pattern_own_colours;
        if ui.checkbox(&mut own, "Its own colours").on_hover_text("Otherwise the brush colour").changed() {
            lines.push(format!("pattern colours {}", if own { "own" } else { "brush" }));
        }
        for line in lines {
            let message = self.pattern_command(&line);
            self.console_log.push(message);
        }
    }
}

fn load_pattern(path: &str) -> Result<Volume, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let file = vox_format::VoxFile::parse(&bytes).map_err(|e| format!("{path}: {e}"))?;
    let model = file.models.first().ok_or_else(|| format!("{path}: no models"))?;
    Ok(Volume::from_model(model))
}
