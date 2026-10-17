//! The Edit panel's model operations, under the reference editor's command names
//! (see `voxel_core::kernels` for what each does):
//!
//! | Command | |
//! |---|---|
//! | `full`, `zero`, `rev` | fill the box, clear it, swap solid and empty |
//! | `x2`, `xh` | double, halve |
//! | `loop <axis> <offset>` | move, wrapping at the edges |
//! | `scale <axes> <factor>` | `scale xyz 2`, `scale z 0.5` |
//! | `shear <axis> <a> <b>` | `shear z 0.2 0.2` |
//! | `rot <axis> <degrees>` | any angle; `rotate x\|y\|z` stays the quarter turn |
//! | `repeat <axes> <count>` | `repeat xy 2` |
//! | `mir <axis>`, `dia <axis>` | make symmetric across the middle, or the diagonal |
//! | `prim elli\|cyli\|pyra\|cone` | a solid filling the box |
//! | `flood [colour]`, `hull` | fill enclosed space; `flood 0` (or `hull`) removes hidden voxels |
//! | `dil [6\|18\|26]`, `ero [6\|18\|26]` | grow or wear by one voxel |
//! | `mask`, `tex` | keep, or repaint, the voxels under the pattern tiled over the box |
//!
//! `flip` and `rot90` are accepted for `mirror` and `rotate` (see `main`).
//!
//! Operations that keep the box are cell writes, so with voxels selected on
//! the model only they change; the others replace the model, keeping its
//! centre in place. Either way it is one undo step.

use crate::App;
use eframe::egui;
use glam::{IVec3, Vec3};
use voxel_core::kernels::{self, Prim};
use voxel_core::Volume;

/// The command words this module answers.
pub const WORDS: [&str; 19] = [
    "full", "zero", "rev", "x2", "xh", "loop", "scale", "shear", "rot", "repeat", "mir", "dia", "prim", "flood",
    "hull", "dil", "ero", "mask", "tex",
];

/// The panel's text fields, which start with the reference editor's examples.
pub struct Fields {
    loop_: String,
    scale: String,
    shear: String,
    rot: String,
    repeat: String,
}

impl Default for Fields {
    fn default() -> Self {
        Self {
            loop_: "z 2".to_owned(),
            scale: "xyz 2".to_owned(),
            shear: "z 0.2 0.2".to_owned(),
            rot: "z 45".to_owned(),
            repeat: "xy 2".to_owned(),
        }
    }
}

fn axis(word: Option<&&str>) -> Result<usize, String> {
    match word.copied() {
        Some("x") => Ok(0),
        Some("y") => Ok(1),
        Some("z") => Ok(2),
        _ => Err("an axis: x, y or z".to_owned()),
    }
}

/// `xyz`, `xy`, `z`...: which axes a factor applies to.
fn axes(word: Option<&&str>) -> Result<[bool; 3], String> {
    let w = word.copied().unwrap_or("");
    if w.is_empty() || !w.chars().all(|c| matches!(c, 'x' | 'y' | 'z')) {
        return Err("axes such as x, xy or xyz".to_owned());
    }
    Ok(['x', 'y', 'z'].map(|a| w.contains(a)))
}

fn number(word: Option<&&str>, what: &str) -> Result<f32, String> {
    word.and_then(|w| w.parse::<f32>().ok())
        .filter(|n| n.is_finite())
        .ok_or_else(|| format!("{what}: a number"))
}

/// What a command does to the model.
enum Edit {
    /// Same box: the new contents, applied as cell writes.
    InPlace(Volume),
    /// A new box.
    Replace(Volume),
}

impl App {
    pub fn edit_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        let verb = words[0];
        if verb == "hull" {
            return self.edit_command("flood 0");
        }
        let Some(model) = self.selected_model() else { return "no model selected".to_owned() };
        let Some(volume) = self.scene.as_ref().and_then(|s| s.doc.scene.models.get(model)).cloned() else {
            return "no model selected".to_owned();
        };
        let colour = self.brush_color;
        let args = &words[1..];
        let edit = match verb {
            "mask" | "tex" => match &self.pattern {
                Some(p) => Ok(Edit::InPlace(if verb == "mask" { kernels::mask(&volume, p) } else { kernels::texture(&volume, p) })),
                None => Err("no pattern: make one from a selection, or open one (pattern …)".to_owned()),
            },
            _ => edit_for(verb, args, &volume, colour),
        };
        let edit = match edit {
            Ok(e) => e,
            Err(e) => return format!("{verb}: {e}"),
        };
        match edit {
            Edit::Replace(after) => self.replace_model(verb, crate::select::Anchor::Centre, |_| Some(after)),
            Edit::InPlace(after) => {
                let what = if verb == "flood" && args.first() == Some(&"0") { "hull" } else { verb };
                self.write_changes(what, model, &volume, &after)
            }
        }
    }

    /// Applies the cells where `after` differs from `before`, only the
    /// selected ones if voxels on this model are selected.
    fn write_changes(&mut self, what: &str, model: usize, before: &Volume, after: &Volume) -> String {
        let only = self.active_selection().filter(|s| s.model == model).map(|s| s.cells().clone());
        let [sx, sy, sz] = before.size();
        let mut writes = Vec::new();
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let new = after.get(x, y, z);
                    let at = [x as i32, y as i32, z as i32];
                    if before.get(x, y, z) != new && only.as_ref().is_none_or(|cells| cells.contains(&at)) {
                        writes.push((IVec3::from_array(at), new));
                    }
                }
            }
        }
        let count = writes.len();
        if count == 0 {
            return format!("{what}: nothing changed");
        }
        let Some(loaded) = self.scene.as_mut() else { return "no model selected".to_owned() };
        self.history.end_gesture();
        self.history.set_voxels(&mut loaded.doc, model, writes);
        self.history.end_gesture();
        self.mesh_dirty = true;
        format!("{what}: {count} voxels changed")
    }

    pub fn edit_ui(&mut self, ui: &mut egui::Ui) {
        let mut lines: Vec<String> = Vec::new();
        fn row(ui: &mut egui::Ui, buttons: &[(&str, &str, &str)], lines: &mut Vec<String>) {
            ui.horizontal_wrapped(|ui| {
                for (label, line, hint) in buttons {
                    if ui.small_button(*label).on_hover_text(*hint).clicked() {
                        lines.push((*line).to_owned());
                    }
                }
            });
        }
        row(
            ui,
            &[
                ("Full", "full", "Fill the whole box with the colour"),
                ("Fill", "fill", "Repaint every voxel with the colour"),
                ("Clear", "zero", "Remove every voxel"),
                ("Reverse", "rev", "Swap solid and empty: empty space takes the colour"),
                ("2X", "x2", "Double the size"),
                ("1/2", "xh", "Halve the size"),
            ],
            &mut lines,
        );
        ui.horizontal(|ui| {
            ui.label("Loop");
            for a in ["x", "y", "z"] {
                for (sign, step) in [("+", 1), ("-", -1)] {
                    if ui.small_button(format!("{sign}{}", a.to_uppercase())).on_hover_text("Move, wrapping at the edges").clicked() {
                        lines.push(format!("loop {a} {step}"));
                    }
                }
            }
        });
        let fields = &mut self.edit_fields;
        for (label, verb, text, hint) in [
            ("Loop", "loop", &mut fields.loop_, "axis offset"),
            ("Scale", "scale", &mut fields.scale, "axes factor"),
            ("Shear", "shear", &mut fields.shear, "axis a b: layers across the axis slide by depth × a, b"),
            ("Rotate", "rot", &mut fields.rot, "axis degrees"),
            ("Repeat", "repeat", &mut fields.repeat, "axes count"),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized([46.0, 18.0], egui::Label::new(label));
                let field = ui.add(egui::TextEdit::singleline(text).desired_width(90.0)).on_hover_text(hint);
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.small_button("Apply").clicked() || entered {
                    lines.push(format!("{verb} {text}"));
                }
            });
        }
        for (label, verb, hint) in [
            ("Mirror", "mir", "Copy the lower half onto the upper half, reflected"),
            ("Diagonal", "dia", "Copy one side of the diagonal across the axis onto the other"),
        ] {
            ui.horizontal(|ui| {
                ui.add_sized([46.0, 18.0], egui::Label::new(label));
                for a in ["x", "y", "z"] {
                    if ui.small_button(a).on_hover_text(hint).clicked() {
                        lines.push(format!("{verb} {a}"));
                    }
                }
            });
        }
        row(
            ui,
            &[
                ("Ellipsoid", "prim elli", "Fill the box with an ellipsoid"),
                ("Cylinder", "prim cyli", "Fill the box with a cylinder up z"),
                ("Pyramid", "prim pyra", "Fill the box with a pyramid"),
                ("Cone", "prim cone", "Fill the box with a cone"),
            ],
            &mut lines,
        );
        row(
            ui,
            &[
                ("Hull", "flood 0", "Remove every voxel that can't be seen"),
                ("Flood", "flood", "Fill enclosed empty space with the colour"),
                ("Dilation", "dil", "Grow by a voxel, in the colour"),
                ("Erosion", "ero", "Wear away a voxel"),
                ("Mask", "mask", "Keep only the voxels under the pattern, tiled over the box"),
                ("Texture", "tex", "Repaint the voxels under the tiled pattern with its colours"),
            ],
            &mut lines,
        );
        for line in lines {
            let message = self.edit_command(&line);
            self.console_log.push(message);
        }
    }
}

fn edit_for(verb: &str, args: &[&str], volume: &Volume, colour: u8) -> Result<Edit, String> {
    use Edit::*;
    Ok(match verb {
        "full" => InPlace(kernels::full(volume, colour)),
        // The console's own `fill <c>` does the same; this is the panel's.
        "fill" => InPlace(kernels::recolour(volume, colour)),
        "zero" => InPlace(Volume::new(volume.size())),
        "rev" => InPlace(kernels::reverse(volume, colour)),
        "x2" => Replace(kernels::double(volume)),
        "xh" => Replace(kernels::halve(volume)),
        "loop" => {
            let a = axis(args.first())?;
            let offset = number(args.get(1), "offset")?;
            InPlace(kernels::shift_wrap(volume, a, offset.round() as i32))
        }
        "scale" => {
            let on = axes(args.first())?;
            let factor = number(args.get(1), "factor")?;
            if factor <= 0.0 {
                return Err("the factor must be above 0".to_owned());
            }
            let s = Vec3::from_array(on.map(|o| if o { factor } else { 1.0 }));
            Replace(kernels::scale(volume, s))
        }
        "shear" => {
            let a = axis(args.first())?;
            let amounts = [number(args.get(1), "first amount")?, number(args.get(2), "second amount").unwrap_or(0.0)];
            InPlace(kernels::shear(volume, a, amounts))
        }
        "rot" => {
            let a = axis(args.first())?;
            Replace(kernels::rotate_by(volume, a, number(args.get(1), "angle")?))
        }
        "repeat" => {
            let on = axes(args.first())?;
            let count = number(args.get(1), "count")?.round();
            if !(1.0..=256.0).contains(&count) {
                return Err("the count must be 1 to 256".to_owned());
            }
            Replace(kernels::repeat(volume, on.map(|o| if o { count as u32 } else { 1 })))
        }
        "mir" => InPlace(kernels::symmetrize(volume, axis(args.first())?)),
        "dia" => InPlace(kernels::diagonal(volume, axis(args.first())?)),
        "prim" => {
            let names: Vec<&str> = Prim::ALL.iter().map(|(_, n)| *n).collect();
            let p = args.first().and_then(|n| Prim::parse(n)).ok_or_else(|| names.join(", "))?;
            InPlace(kernels::prim(volume, p, colour))
        }
        "flood" => match args.first() {
            Some(c) => match c.parse::<u8>() {
                Ok(0) => InPlace(kernels::hull(volume)),
                Ok(c) => InPlace(kernels::flood(volume, c)),
                Err(_) => return Err("a colour, 0 to 255".to_owned()),
            },
            None => InPlace(kernels::flood(volume, colour)),
        },
        "dil" | "ero" => {
            let n = match args.first() {
                Some(w) => w.parse::<u32>().ok().and_then(kernels::neighbours),
                None => kernels::neighbours(6),
            }
            .ok_or("neighbours: 6, 18 or 26")?;
            if verb == "dil" {
                InPlace(kernels::dilate(volume, colour, &n))
            } else {
                InPlace(kernels::erode(volume, &n))
            }
        }
        _ => return Err("unknown".to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_checked() {
        let v = Volume::new([4, 4, 4]);
        assert!(edit_for("loop", &["w", "1"], &v, 1).is_err());
        assert!(edit_for("scale", &["xyz", "0"], &v, 1).is_err());
        assert!(edit_for("scale", &["xq", "2"], &v, 1).is_err());
        assert!(edit_for("repeat", &["x", "0"], &v, 1).is_err());
        assert!(edit_for("prim", &["blob"], &v, 1).is_err());
        assert!(edit_for("dil", &["7"], &v, 1).is_err());
        assert!(matches!(edit_for("scale", &["z", "2"], &v, 1), Ok(Edit::Replace(s)) if s.size() == [4, 4, 8]));
        assert!(matches!(edit_for("shear", &["z", "0.5"], &v, 1), Ok(Edit::InPlace(_))));
    }

    #[test]
    fn every_word_is_handled() {
        let v = Volume::new([4, 4, 4]);
        for w in WORDS {
            if matches!(w, "hull" | "mask" | "tex") {
                continue;
            }
            let args: &[&str] = match w {
                "loop" | "rot" => &["z", "1"],
                "scale" | "repeat" => &["x", "2"],
                "shear" => &["z", "0.2", "0.2"],
                "mir" | "dia" => &["x"],
                "prim" => &["cone"],
                _ => &[],
            };
            assert!(edit_for(w, args, &v, 1).is_ok(), "{w}");
        }
    }
}
