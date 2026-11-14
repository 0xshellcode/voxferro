//! The Edit panel's model operations, under their command names
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
use glam::Vec3;
use voxel_core::kernels::{self, Prim};
use voxel_core::Volume;

/// The command words this module answers.
pub const WORDS: [&str; 22] = [
    "full", "zero", "rev", "x2", "xh", "loop", "scale", "shear", "rot", "repeat", "mir", "dia", "prim", "flood",
    "hull", "dil", "ero", "mask", "tex", "maze", "noise", "fractal",
];

/// The panel's text fields, which start with examples.
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
            "mask" | "tex" | "fractal" => match &self.pattern {
                Some(p) if verb == "fractal" => Ok(Edit::Replace(kernels::fractal(&volume, p))),
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
        let Some(loaded) = self.scene.as_mut() else { return "no model selected".to_owned() };
        debug_assert_eq!(loaded.doc.scene.models.get(model).map(Volume::size), Some(before.size()));
        let command = voxel_core::Command::diff(&loaded.doc, model, after, |p| {
            only.as_ref().is_none_or(|cells| cells.contains(&p.to_array()))
        });
        let count = match &command {
            voxel_core::Command::SetVoxels { changes, .. } => changes.len(),
            _ => 0,
        };
        if count == 0 {
            return format!("{what}: nothing changed");
        }
        self.history.exec_alone(&mut loaded.doc, command);
        self.mesh_dirty = true;
        format!("{what}: {count} voxels changed")
    }

    /// The Edit panel: Tool and Select open, the rest folded.
    pub fn edit_panel(&mut self, ui: &mut egui::Ui) {
        crate::layout::title(ui, "Edit");
        let mut lines: Vec<String> = Vec::new();
        /// A row of buttons filling the width, each running a command.
        fn row(ui: &mut egui::Ui, buttons: &[(&str, &str, &str)], lines: &mut Vec<String>) {
            let labels: Vec<(&str, &str)> = buttons.iter().map(|(l, _, h)| (*l, *h)).collect();
            if let Some(i) = crate::layout::button_row(ui, &labels) {
                lines.push(buttons[i].1.to_owned());
            }
        }
        /// A text field and Apply, for the numeric sections.
        fn field(ui: &mut egui::Ui, verb: &str, text: &mut String, hint: &str, lines: &mut Vec<String>) {
            ui.horizontal(|ui| {
                let response = ui.add(egui::TextEdit::singleline(text).desired_width(100.0)).on_hover_text(hint);
                let entered = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.small_button("Apply").clicked() || entered {
                    lines.push(format!("{verb} {text}"));
                }
            });
        }
        fn axes(ui: &mut egui::Ui, verb: &str, hint: &str, lines: &mut Vec<String>) {
            if let Some(i) = crate::layout::button_row(ui, &[("X", hint), ("Y", hint), ("Z", hint)]) {
                lines.push(format!("{verb} {}", ["x", "y", "z"][i]));
            }
        }
        let section = |name: &str, open: bool| {
            egui::CollapsingHeader::new(egui::RichText::new(name).size(10.5)).id_salt(("edit", name)).default_open(open)
        };

        // In the Move tool, the World-mode object panel first.
        if self.moving {
            section("OBJECT", true).show(ui, |ui| {
                let n = self.object_indices().len();
                ui.label(egui::RichText::new(format!("{n} selected, {}  ·  Shift+click adds", self.level_name())).size(10.5).weak());
                row(ui, &[("All", "object all", "Select every object (Cmd+A)"), ("Inv", "object invert", "Select the others (Cmd+I)"), ("None", "object none", "Clear the selection (Cmd+D)")], &mut lines);
                row(ui, &[("Copy", "object copy", "Copy, each with a model of its own"), ("Ref", "object copy ref", "Copy as references to the same models"), ("Cut", "object cut", "Copy as references and delete"), ("Paste", "object paste", "Paste where they were copied from")], &mut lines);
                row(ui, &[("Union", "object union", "Fuse into one model; the last selected wins where they overlap"), ("Inter", "object intersect", "Keep only where every object is filled"), ("Oper", "object operand", "Set the selection aside to subtract or paint with")], &mut lines);
                row(ui, &[("Subtract", "object subtract", "Remove the operand's voxels from the selected objects"), ("Replace", "object replace", "Paint the selected objects in the operand's colours where it is")], &mut lines);
                for (side, label) in [(1, "+"), (0, "c"), (-1, "-")] {
                    let names = ["X", "Y", "Z"].map(|a| format!("{label}{a}"));
                    let hint = match side {
                        1 => "Align each object's high side to the selection's",
                        0 => "Centre each object on the selection's centre",
                        _ => "Align each object's low side to the selection's",
                    };
                    if let Some(i) = crate::layout::button_row(ui, &[(&names[0], hint), (&names[1], hint), (&names[2], hint)]) {
                        lines.push(format!("object align {} {side}", ["x", "y", "z"][i]));
                    }
                }
                row(ui, &[("Flip X", "object flip x", "Mirror the selection across x"), ("Flip Y", "object flip y", "Mirror the selection across y"), ("Flip Z", "object flip z", "Mirror the selection across z")], &mut lines);
                row(ui, &[("Ground", "object ground", "Move the selection down onto z = 0"), ("Own", "object own", "Give each selected reference a model of its own"), ("Delete", "object delete", "Delete the selected objects")], &mut lines);
                row(ui, &[("T copy", "object transform copy", "Copy the first object's rotation and centre"), ("T paste", "object transform paste", "Give the selection that rotation and centre"), ("Rot", "object transform paste rot", "Paste only the rotation"), ("Pos", "object transform paste pos", "Paste only the position")], &mut lines);
                row(ui, &[("Upright", "object transform reset", "Turn each object back upright, in place"), ("Open (Tab)", "object level in", "Open the selected group: work on its children"), ("Close", "object level out", "Close the open group (Shift+Tab)")], &mut lines);
            });
        }

        section("TOOL", true).show(ui, |ui| {
            row(ui, &[("Full", "full", "Fill the whole box with the colour"), ("Fill", "fill", "Repaint every voxel with the colour")], &mut lines);
            row(ui, &[("Clear", "zero", "Remove every voxel"), ("Rev", "rev", "Swap solid and empty: empty space takes the colour")], &mut lines);
            row(ui, &[("2X", "x2", "Double the size"), ("1/2", "xh", "Halve the size")], &mut lines);
        });
        section("SELECT", true).show(ui, |ui| self.selection_ui(ui));
        section("ROT", false).show(ui, |ui| axes(ui, "rotate", "A quarter turn about the axis", &mut lines));
        section("FLIP", false).show(ui, |ui| axes(ui, "flip", "Flip along the axis", &mut lines));
        section("LOOP", false).show(ui, |ui| {
            for (sign, step) in [("+", 1), ("-", -1)] {
                let labels = ["X", "Y", "Z"].map(|a| format!("{sign}{a}"));
                let hint = "Move, wrapping at the edges";
                if let Some(i) = crate::layout::button_row(ui, &[(&labels[0], hint), (&labels[1], hint), (&labels[2], hint)]) {
                    lines.push(format!("loop {} {step}", ["x", "y", "z"][i]));
                }
            }
            field(ui, "loop", &mut self.edit_fields.loop_, "axis offset", &mut lines);
        });
        section("SCALE", false).show(ui, |ui| field(ui, "scale", &mut self.edit_fields.scale, "axes factor", &mut lines));
        section("SHEAR", false).show(ui, |ui| {
            field(ui, "shear", &mut self.edit_fields.shear, "axis a b: layers across the axis slide by depth × a, b", &mut lines)
        });
        section("ROTATE", false).show(ui, |ui| field(ui, "rot", &mut self.edit_fields.rot, "axis degrees", &mut lines));
        section("REPEAT", false).show(ui, |ui| field(ui, "repeat", &mut self.edit_fields.repeat, "axes count", &mut lines));
        ui.separator();
        section("MIRROR", false).show(ui, |ui| axes(ui, "mir", "Copy the lower half onto the upper half, reflected", &mut lines));
        section("DIAGONAL", false).show(ui, |ui| axes(ui, "dia", "Copy one side of the diagonal across the axis onto the other", &mut lines));
        section("SHAPE", false).show(ui, |ui| {
            row(ui, &[("Ellipsoid", "prim elli", "Fill the box with an ellipsoid"), ("Cylinder", "prim cyli", "Fill the box with a cylinder up z")], &mut lines);
            row(ui, &[("Pyramid", "prim pyra", "Fill the box with a pyramid"), ("Cone", "prim cone", "Fill the box with a cone")], &mut lines);
            row(ui, &[("Maze", "maze", "A new 3D maze of corridors in the colour"), ("Perlin", "noise -1", "Perlin noise in the colour, a new seed each time")], &mut lines);
            row(ui, &[("Fractal", "fractal", "Each voxel becomes the pattern, in its colour")], &mut lines);
        });
        section("MODIFY", false).show(ui, |ui| {
            row(ui, &[("Hull", "flood 0", "Remove every voxel that can't be seen"), ("Flood", "flood", "Fill enclosed empty space with the colour")], &mut lines);
            row(ui, &[("Mask", "mask", "Keep only the voxels under the pattern, tiled over the box"), ("Texture", "tex", "Repaint the voxels under the tiled pattern with its colours")], &mut lines);
            row(ui, &[("Dilation", "dil", "Grow by a voxel, in the colour"), ("Erosion", "ero", "Wear away a voxel")], &mut lines);
        });
        for line in lines {
            // ROT and FLIP are the editor's own `rotate` and `mirror`.
            let message = match line.split_whitespace().next() {
                Some("rotate") => self.select_command(&line, ui.ctx()),
                Some("flip") => self.select_command(&line.replacen("flip", "mirror", 1), ui.ctx()),
                Some("object") => self.object_command(&line),
                _ => self.edit_command(&line),
            };
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
        // Maze and Perlin: a new model in the box's size.
        "maze" => {
            let p = match args.first() {
                Some(n) => n.parse::<u32>().map_err(|_| "maze [corridor gap, 1 or more]".to_owned())?,
                None => 1,
            };
            InPlace(kernels::maze(volume.size(), colour, p, &mut kernels::Rand::from_clock()))
        }
        "noise" => {
            // `noise [seed] [scale] [lo] [hi]`; a seed below 0 is a new one.
            let n = |i: usize, default: f32| args.get(i).map_or(Ok(default), |a| a.parse::<f32>().map_err(|_| "noise [seed] [scale] [lo] [hi]".to_owned()));
            let seed = n(0, 0.0)?;
            let seed = if seed < 0.0 { kernels::Rand::from_clock().next_u32() % 65535 } else { seed.min(65535.0) as u32 };
            InPlace(kernels::noise(volume.size(), colour, seed, n(1, 0.04)?, n(2, 0.2)?, n(3, 0.5)?))
        }
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
            if matches!(w, "hull" | "mask" | "tex" | "fractal") {
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
