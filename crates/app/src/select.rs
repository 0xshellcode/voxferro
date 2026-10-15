//! The Select tool, selection edits, copy and paste, and whole-model
//! operations.

use crate::{App, edit::Ray};
use eframe::egui;
use glam::{IVec3, Mat4, Vec3};
use voxel_core::brush::{self, BrushShape};
use voxel_core::{Combine, Command, Selection, ops};

/// Text put on the system clipboard on copy. The voxels themselves stay in
/// the editor; egui only reports a paste when the clipboard holds text, so
/// this marker is what makes Cmd+V reach us.
const CLIPBOARD_MARKER: &str = "Voxferro voxels";

/// What a model operation keeps fixed in the world.
enum Anchor {
    /// Voxels keep their place; the new box's cell 0 was this old cell.
    Cell(IVec3),
    /// The box's centre: for operations that turn or flip the contents.
    Centre,
}

/// A selection gesture in progress.
pub struct SelectDrag {
    /// Where the press landed, in screen points, for the marquee.
    pub from: egui::Pos2,
    pub to: egui::Pos2,
    /// The instance the gesture selects in.
    instance: usize,
    combine: Combine,
}

impl App {
    /// The model the current selection lives in, if any and non-empty.
    pub fn active_selection(&self) -> Option<&Selection> {
        self.selection.as_ref().filter(|s| !s.is_empty())
    }

    /// Starts a selection gesture. Box marquees start anywhere; the other
    /// shapes need a voxel under the press.
    pub fn begin_select(&mut self, ray: Ray, at: egui::Pos2, modifiers: egui::Modifiers) -> bool {
        let combine = if modifiers.shift {
            Combine::Add
        } else if modifiers.alt {
            Combine::Subtract
        } else {
            Combine::Replace
        };
        let pick = self.pick_any(ray);
        if let Some((instance, _)) = pick {
            self.selected = instance;
        }
        let instance = self.selected;
        match self.brush_shape {
            BrushShape::Box => {
                self.select_drag = Some(SelectDrag { from: at, to: at, instance, combine });
                true
            }
            _ => {
                let Some((instance, hit)) = pick else { return false };
                self.select_drag = Some(SelectDrag { from: at, to: at, instance, combine });
                let cells = self.cells_for_select(hit);
                self.combine_selection(instance, cells, combine);
                true
            }
        }
    }

    pub fn update_select(&mut self, ray: Ray, at: egui::Pos2) {
        let Some(drag) = &mut self.select_drag else { return };
        drag.to = at;
        // Dragging the voxel shape keeps adding what passes under it.
        if matches!(self.brush_shape, BrushShape::Voxel | BrushShape::Line) {
            let instance = drag.instance;
            let combine = if drag.combine == Combine::Subtract { Combine::Subtract } else { Combine::Add };
            if let Some((i, hit)) = self.pick_any(ray) {
                if i == instance {
                    self.combine_selection(instance, vec![hit.voxel], combine);
                }
            }
        }
    }

    /// Ends the gesture; a marquee selects every voxel whose centre
    /// projects inside it, through the whole depth of the model.
    pub fn finish_select(&mut self, view_proj: Mat4, viewport: egui::Rect) {
        let Some(drag) = self.select_drag.take() else { return };
        if self.brush_shape != BrushShape::Box {
            return;
        }
        let rect = egui::Rect::from_two_pos(drag.from, drag.to);
        let Some(loaded) = &self.scene else { return };
        let Some(instance) = loaded.doc.scene.instances.get(drag.instance) else { return };
        let Some(volume) = loaded.doc.scene.models.get(instance.model_index) else { return };

        let [sx, sy, sz] = volume.size();
        let mut cells = Vec::new();
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    if volume.get(x, y, z) == 0 {
                        continue;
                    }
                    let local = Vec3::new(x as f32, y as f32, z as f32) + Vec3::splat(0.5);
                    let world = instance.transform.apply(local - volume.pivot());
                    let clip = view_proj * world.extend(1.0);
                    if clip.w <= 0.0 {
                        continue;
                    }
                    let ndc = clip.truncate() / clip.w;
                    let screen = viewport.min
                        + egui::vec2(
                            (ndc.x + 1.0) * 0.5 * viewport.width(),
                            (1.0 - ndc.y) * 0.5 * viewport.height(),
                        );
                    if rect.contains(screen) {
                        cells.push(IVec3::new(x as i32, y as i32, z as i32));
                    }
                }
            }
        }
        self.combine_selection(drag.instance, cells, drag.combine);
    }

    fn cells_for_select(&self, hit: voxel_core::Hit) -> Vec<IVec3> {
        let Some(volume) = self.selected_model().and_then(|m| self.scene.as_ref()?.doc.scene.models.get(m)) else {
            return Vec::new();
        };
        match self.brush_shape {
            BrushShape::Face => brush::face_region(volume, hit.voxel, hit.face),
            BrushShape::Fill => brush::colour_region(volume, hit.voxel),
            _ => vec![hit.voxel],
        }
    }

    fn combine_selection(&mut self, instance: usize, cells: Vec<IVec3>, combine: Combine) {
        let Some(loaded) = &self.scene else { return };
        let Some(model) = loaded.doc.scene.instances.get(instance).map(|i| i.model_index) else { return };
        let volume = &loaded.doc.scene.models[model];
        let selection = match self.selection.take() {
            Some(s) if s.model == model => s,
            _ => Selection::new(model),
        };
        let mut selection = selection;
        selection.combine(volume, cells, combine);
        self.selection = Some(selection);
        self.mesh_dirty = true;
    }

    fn pick_any(&self, ray: Ray) -> Option<(usize, voxel_core::Hit)> {
        let loaded = self.scene.as_ref()?;
        let visible = self.visibility();
        let pick = loaded
            .doc
            .scene
            .pick(ray.origin, ray.direction, 10_000.0, |i| visible.shows(i))?;
        Some((pick.instance, pick.hit))
    }

    /// Keeps the selection consistent after the document changed under it:
    /// voxels it named may be gone, or the model may have changed shape.
    pub fn revalidate_selection(&mut self) {
        let Some(selection) = &mut self.selection else { return };
        let Some(volume) = self.scene.as_ref().and_then(|s| s.doc.scene.models.get(selection.model)) else {
            self.selection = None;
            return;
        };
        selection.retain_filled(volume);
    }

    /// Runs writes against the selection's model as one undo step.
    fn exec_on_selection(&mut self, writes: Vec<(IVec3, u8)>) -> usize {
        let Some(model) = self.selection.as_ref().map(|s| s.model) else { return 0 };
        let Some(loaded) = self.scene.as_mut() else { return 0 };
        self.history.end_gesture();
        let n = self.history.set_voxels(&mut loaded.doc, model, writes);
        self.history.end_gesture();
        self.mesh_dirty = true;
        n
    }

    /// Swaps the selected model for `op` of it, as one undo step.
    /// Swaps the selected model for `op` of it, as one undo step, and
    /// moves its instances so the voxels stay where they were in the world.
    ///
    /// A model is placed by its pivot, `floor(size / 2)`, so any change of
    /// size moves it unless the placement compensates. `anchor` says which
    /// point must not move.
    fn replace_model(
        &mut self,
        what: &str,
        anchor: Anchor,
        op: impl FnOnce(&voxel_core::Volume) -> Option<voxel_core::Volume>,
    ) -> String {
        let Some(model) = self.selected_model() else { return "no model selected".to_owned() };
        let Some(loaded) = self.scene.as_mut() else { return "no model selected".to_owned() };
        let before = loaded.doc.scene.models[model].clone();
        let Some(after) = op(&before) else { return format!("{what}: nothing to do") };
        let [x, y, z] = after.size();

        let (p0, p1) = (before.pivot(), after.pivot());
        let size = |v: &voxel_core::Volume| Vec3::from_array(v.size().map(|s| s as f32));
        // The shift, in the model's own axes, that keeps the anchor fixed.
        let delta = match anchor {
            // New cell 0 was old cell `origin`: t' = t + R (origin + p1 - p0).
            Anchor::Cell(origin) => origin.as_vec3() + p1 - p0,
            // The box centre stays: t' = t + R (c0 - c1).
            Anchor::Centre => (size(&before) * 0.5 - p0) - (size(&after) * 0.5 - p1),
        }
        .round()
        .as_ivec3();

        let mut commands = vec![Command::ReplaceModel { model, before, after }];
        if delta != IVec3::ZERO {
            let nodes = loaded.doc.scene.with_model_shifted(model, delta);
            commands.push(Command::SetGraph { before: loaded.doc.scene.nodes.clone(), after: nodes });
        }
        if !self.history.exec_alone(&mut loaded.doc, Command::Batch(commands)) {
            return format!("{what}: nothing changed");
        }
        // Coordinates mean something else in the new shape.
        self.selection = None;
        self.mesh_dirty = true;
        format!("{what}: model is now {x}x{y}x{z}")
    }

    /// Selection and model commands, from the console, the panel and the
    /// keyboard alike.
    pub fn select_command(&mut self, line: &str, ctx: &egui::Context) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        let axis = |w: Option<&&str>| match w.copied() {
            Some("x") => Some(0),
            Some("y") => Some(1),
            Some("z") => Some(2),
            _ => None,
        };
        let model_volume = |app: &App| {
            let m = app.selected_model()?;
            Some((m, app.scene.as_ref()?.doc.scene.models.get(m)?.clone()))
        };

        match words.as_slice() {
            ["select", rest @ ..] => {
                let Some((model, volume)) = model_volume(self) else { return "no model selected".to_owned() };
                let next = match rest {
                    ["all"] => Selection::all(model, &volume),
                    ["none"] => Selection::new(model),
                    ["invert"] => match &self.selection {
                        Some(s) if s.model == model => s.inverted(&volume),
                        _ => Selection::all(model, &volume),
                    },
                    ["color", c] => match c.parse::<u8>() {
                        Ok(c) => Selection::by_colour(model, &volume, c),
                        Err(_) => return "select color <1-255>".to_owned(),
                    },
                    ["color"] => Selection::by_colour(model, &volume, self.brush_color),
                    _ => return "select all | none | invert | color [n]".to_owned(),
                };
                let n = next.len();
                self.selection = Some(next);
                self.mesh_dirty = true;
                format!("{n} voxels selected")
            }
            ["delete"] => {
                let Some(s) = self.active_selection() else { return "nothing selected".to_owned() };
                let writes = s.erase_writes();
                let n = self.exec_on_selection(writes);
                self.selection = self.selection.as_ref().map(|s| Selection::new(s.model));
                format!("deleted {n} voxels")
            }
            ["paint", rest @ ..] => {
                let colour = rest.first().and_then(|c| c.parse::<u8>().ok()).unwrap_or(self.brush_color);
                let Some(s) = self.active_selection() else { return "nothing selected".to_owned() };
                let writes = s.paint_writes(colour);
                let n = self.exec_on_selection(writes);
                format!("painted {n} voxels")
            }
            ["copy"] | ["cut"] => {
                let Some(s) = self.active_selection() else { return "nothing selected".to_owned() };
                let Some(volume) = self.scene.as_ref().and_then(|l| l.doc.scene.models.get(s.model)) else {
                    return "nothing selected".to_owned();
                };
                self.clipboard = s.copy(volume);
                ctx.copy_text(CLIPBOARD_MARKER.to_owned());
                let n = self.clipboard.voxels.len();
                if words[0] == "cut" {
                    let writes = self.active_selection().map(|s| s.erase_writes()).unwrap_or_default();
                    self.exec_on_selection(writes);
                    self.selection = self.selection.as_ref().map(|s| Selection::new(s.model));
                    format!("cut {n} voxels")
                } else {
                    format!("copied {n} voxels")
                }
            }
            ["paste"] => {
                if self.clipboard.is_empty() {
                    return "nothing to paste".to_owned();
                }
                let Some((model, volume)) = model_volume(self) else { return "no model selected".to_owned() };
                let (writes, pasted) = self.clipboard.paste_writes(model, &volume);
                self.selection = Some(pasted);
                let n = self.exec_on_selection(writes);
                format!("pasted {n} voxels")
            }
            ["move", dx, dy, dz] => {
                let (Ok(dx), Ok(dy), Ok(dz)) = (dx.parse(), dy.parse(), dz.parse()) else {
                    return "move <dx> <dy> <dz>".to_owned();
                };
                self.move_selection(IVec3::new(dx, dy, dz))
            }
            ["rotate", a] => match axis(Some(a)) {
                Some(axis) => self.replace_model("rotate", Anchor::Centre, |v| Some(ops::rotate(v, axis))),
                None => "rotate x|y|z".to_owned(),
            },
            ["mirror", a] => match axis(Some(a)) {
                Some(axis) => self.replace_model("mirror", Anchor::Centre, |v| Some(ops::mirror(v, axis))),
                None => "mirror x|y|z".to_owned(),
            },
            ["crop"] => {
                let lo = self
                    .selected_model()
                    .and_then(|m| self.scene.as_ref()?.doc.scene.models.get(m)?.occupied_bounds())
                    .map_or(IVec3::ZERO, |(lo, _)| IVec3::new(lo[0] as i32, lo[1] as i32, lo[2] as i32));
                self.replace_model("crop", Anchor::Cell(lo), ops::crop)
            }
            ["resize", x, y, z] => match (x.parse::<u32>(), y.parse::<u32>(), z.parse::<u32>()) {
                (Ok(x), Ok(y), Ok(z)) if [x, y, z].iter().all(|s| (1..=256).contains(s)) => {
                    self.replace_model("resize", Anchor::Cell(IVec3::ZERO), |v| Some(ops::resize(v, [x, y, z])))
                }
                _ => "resize <x> <y> <z>, each 1 to 256".to_owned(),
            },
            _ => "select | delete | paint | copy | cut | paste | move | rotate | mirror | crop | resize".to_owned(),
        }
    }

    fn move_selection(&mut self, delta: IVec3) -> String {
        let Some(s) = self.active_selection() else { return "nothing selected".to_owned() };
        let Some(volume) = self.scene.as_ref().and_then(|l| l.doc.scene.models.get(s.model)) else {
            return "nothing selected".to_owned();
        };
        let (writes, moved) = s.move_writes(volume, delta);
        self.selection = Some(moved);
        self.exec_on_selection(writes);
        format!("moved by {} {} {}", delta.x, delta.y, delta.z)
    }

    /// Selection shortcuts. Skipped while a text field has focus, so typing
    /// in the console never deletes voxels.
    pub fn selection_input(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let mut commands: Vec<&str> = Vec::new();
        let mut delta = IVec3::ZERO;
        ctx.input(|i| {
            let cmd = i.modifiers.command;
            if cmd && i.key_pressed(egui::Key::A) {
                commands.push("select all");
            }
            if (cmd && i.key_pressed(egui::Key::D)) || i.key_pressed(egui::Key::Escape) {
                commands.push("select none");
            }
            if i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace) {
                commands.push("delete");
            }
            for event in &i.events {
                match event {
                    egui::Event::Copy => commands.push("copy"),
                    egui::Event::Cut => commands.push("cut"),
                    egui::Event::Paste(text) if text == CLIPBOARD_MARKER => commands.push("paste"),
                    _ => {}
                }
            }
            for (key, step) in [
                (egui::Key::ArrowLeft, -IVec3::X),
                (egui::Key::ArrowRight, IVec3::X),
                (egui::Key::ArrowDown, -IVec3::Y),
                (egui::Key::ArrowUp, IVec3::Y),
                (egui::Key::PageDown, -IVec3::Z),
                (egui::Key::PageUp, IVec3::Z),
            ] {
                if i.key_pressed(key) {
                    delta += step;
                }
            }
        });
        for c in commands {
            let message = self.select_command(c, ctx);
            self.console_log.push(message);
        }
        if delta != IVec3::ZERO && self.active_selection().is_some() {
            let message = self.move_selection(delta);
            self.console_log.push(message);
        }
    }

    /// The Selection and Model panel sections.
    pub fn selection_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let run = |app: &mut App, line: &str| {
            let message = app.select_command(line, &ctx);
            app.console_log.push(message);
        };

        ui.heading("Selection");
        let count = self.active_selection().map_or(0, Selection::len);
        ui.label(format!("{count} voxels selected"));
        ui.horizontal_wrapped(|ui| {
            for (label, line) in [("All", "select all"), ("None", "select none"), ("Invert", "select invert"), ("Colour", "select color")] {
                if ui.small_button(label).clicked() {
                    run(self, line);
                }
            }
        });
        ui.add_enabled_ui(count > 0, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (label, line) in [("Delete", "delete"), ("Paint", "paint"), ("Copy", "copy"), ("Cut", "cut")] {
                    if ui.small_button(label).clicked() {
                        run(self, line);
                    }
                }
            });
        });
        if ui.add_enabled(!self.clipboard.is_empty(), egui::Button::new("Paste").small()).clicked() {
            run(self, "paste");
        }

        ui.heading("Model");
        // Fill the Resize fields from the model whenever it, or its size,
        // changes; leave them alone while the user types.
        let current = self.selected_model().and_then(|m| {
            Some((m, self.scene.as_ref()?.doc.scene.models.get(m)?.size()))
        });
        if current.is_some() && current != self.resize_for {
            self.resize_for = current;
            if let Some((_, size)) = current {
                self.resize_to = size;
            }
        }
        ui.horizontal(|ui| {
            ui.label("Rotate");
            for a in ["x", "y", "z"] {
                if ui.small_button(a).clicked() {
                    run(self, &format!("rotate {a}"));
                }
            }
            ui.label("Mirror");
            for a in ["x", "y", "z"] {
                if ui.small_button(a).clicked() {
                    run(self, &format!("mirror {a}"));
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Crop").clicked() {
                run(self, "crop");
            }
            for v in &mut self.resize_to {
                ui.add(egui::DragValue::new(v).range(1..=256));
            }
            if ui.small_button("Resize").clicked() {
                let [x, y, z] = self.resize_to;
                run(self, &format!("resize {x} {y} {z}"));
            }
        });
    }
}
