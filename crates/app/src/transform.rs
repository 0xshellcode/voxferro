//! The Transform tool: moving, scaling and turning the selected voxels.
//!
//! the reference editor's Transform options (`m_brush.ui`) are move/loop, scale, wrap
//! and rotate, done with the mouse and committed with Enter. Moving is the
//! editor's `move` (and the arrow keys); here are scale and rotate, as
//! commands and panel buttons:
//!
//! - `transform scale <axes> <factor>`: `transform scale xyz 2`;
//! - `transform rotate <axis> <degrees>`: any angle.
//!
//! And with the mouse, as the reference editor's tool works: choose Move, Scale or
//! Turn, then drag in the viewport -- right and left (and for Move, up and
//! down) -- watching a preview; Enter keeps it as one undo step, Esc drops
//! it. Shift snaps a turn to 15°.
//!
//! The selected voxels are lifted out into their own box, transformed with
//! the Edit panel's kernels (nearest cell, as there), and put back centred
//! where they were; what falls outside the model is cut off. The old cells
//! are cleared, the new ones written over whatever is there, and the
//! selection follows -- one undo step.

use crate::App;
use glam::{IVec3, Vec3};
use std::collections::BTreeMap;
use voxel_core::{Selection, Volume, kernels};

/// The selected voxels of `volume` in a box of their own, and the box's
/// corner in the model.
fn lift(volume: &Volume, selection: &Selection) -> Option<(Volume, IVec3)> {
    let solid: Vec<IVec3> = selection.iter().filter(|p| volume.get_i(p.x, p.y, p.z) != 0).collect();
    let first = *solid.first()?;
    let (lo, hi) = solid.iter().fold((first, first), |(a, b), &c| (a.min(c), b.max(c)));
    let size = (hi - lo + IVec3::ONE).as_uvec3().to_array();
    let mut out = Volume::new(size);
    for p in solid {
        let q = (p - lo).as_uvec3();
        out.set(q.x, q.y, q.z, volume.get_i(p.x, p.y, p.z));
    }
    Some((out, lo))
}

/// The writes that replace the lifted voxels by `after`, centred where the
/// lifted box was, and the cells `after` fills.
pub fn place(volume: &Volume, selection: &Selection, before_size: [u32; 3], corner: IVec3, after: &Volume) -> (Vec<(IVec3, u8)>, Vec<IVec3>) {
    place_moved(volume, selection, before_size, corner, after, IVec3::ZERO)
}

/// `place`, shifted by `offset` cells.
pub fn place_moved(
    volume: &Volume,
    selection: &Selection,
    before_size: [u32; 3],
    corner: IVec3,
    after: &Volume,
    offset: IVec3,
) -> (Vec<(IVec3, u8)>, Vec<IVec3>) {
    let mut writes: BTreeMap<[i32; 3], u8> = BTreeMap::new();
    for p in selection.iter().filter(|p| volume.get_i(p.x, p.y, p.z) != 0) {
        writes.insert(p.to_array(), 0);
    }
    let centre = corner.as_vec3() + Vec3::from_array(before_size.map(|s| s as f32)) * 0.5;
    let new_corner = (centre - Vec3::from_array(after.size().map(|s| s as f32)) * 0.5).round().as_ivec3() + offset;
    let [sx, sy, sz] = volume.size();
    let inside = |p: IVec3| p.x >= 0 && p.y >= 0 && p.z >= 0 && (p.x as u32) < sx && (p.y as u32) < sy && (p.z as u32) < sz;
    let mut cells = Vec::new();
    let [ax, ay, az] = after.size();
    for z in 0..az {
        for y in 0..ay {
            for x in 0..ax {
                let v = after.get(x, y, z);
                let p = new_corner + IVec3::new(x as i32, y as i32, z as i32);
                if v != 0 && inside(p) {
                    writes.insert(p.to_array(), v);
                    cells.push(p);
                }
            }
        }
    }
    let writes = writes
        .into_iter()
        .map(|(p, v)| (IVec3::from_array(p), v))
        .filter(|(p, v)| volume.get_i(p.x, p.y, p.z) != *v)
        .collect();
    (writes, cells)
}

/// What a mouse transform does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Move,
    Scale,
    Turn,
}

/// A mouse transform in progress: what was lifted, and the preview applied
/// to the document outside the history.
pub struct TransformDrag {
    handle: Handle,
    start: eframe::egui::Pos2,
    model: usize,
    selection: Selection,
    lifted: Volume,
    corner: IVec3,
    preview: Option<voxel_core::Command>,
    /// The drag so far, so a further drag goes on from it.
    last: eframe::egui::Vec2,
    /// The cells the preview fills, for the selection after Enter.
    cells: Vec<IVec3>,
    pub dragging: bool,
}

/// The world axis a screen direction runs along most, signed.
fn dominant(v: Vec3) -> IVec3 {
    let a = v.abs();
    let axis = if a.x >= a.y && a.x >= a.z { 0 } else if a.y >= a.z { 1 } else { 2 };
    let mut out = IVec3::ZERO;
    out[axis] = if v[axis] >= 0.0 { 1 } else { -1 };
    out
}

impl App {
    /// Starts a mouse transform of the selection, if the tool is armed and
    /// voxels are selected. True if it started.
    pub fn begin_transform(&mut self, at: eframe::egui::Pos2) -> bool {
        let Some(handle) = self.transform_tool else { return false };
        if let Some(drag) = &mut self.transform_drag {
            // A further drag goes on from the preview kept so far.
            drag.dragging = true;
            drag.start = at - drag.last;
            return true;
        }
        let Some(selection) = self.active_selection().cloned() else { return false };
        let Some(volume) = self.scene.as_ref().and_then(|s| s.doc.scene.models.get(selection.model)) else { return false };
        let Some((lifted, corner)) = lift(volume, &selection) else { return false };
        self.transform_drag = Some(TransformDrag {
            handle,
            start: at,
            model: selection.model,
            selection,
            lifted,
            corner,
            preview: None,
            last: eframe::egui::Vec2::ZERO,
            cells: Vec::new(),
            dragging: true,
        });
        true
    }

    /// Follows the cursor: the preview for the drag so far.
    pub fn update_transform(&mut self, at: eframe::egui::Pos2, shift: bool) {
        let basis = self.camera.basis();
        let Some(drag) = &mut self.transform_drag else { return };
        let Some(loaded) = self.scene.as_mut() else { return };
        if let Some(old) = drag.preview.take() {
            old.revert(&mut loaded.doc);
        }
        let d = at - drag.start;
        drag.last = d;
        let (after, offset) = match drag.handle {
            Handle::Move => {
                // A cell per 12 points, along the world axes nearest the
                // screen's right and up.
                let right = dominant(basis.x_axis) * (d.x / 12.0).round() as i32;
                let up = dominant(basis.y_axis) * (-d.y / 12.0).round() as i32;
                (drag.lifted.clone(), right + up)
            }
            Handle::Scale => {
                let f = 2f32.powf(d.x / 150.0);
                (kernels::scale(&drag.lifted, Vec3::splat(f)), IVec3::ZERO)
            }
            Handle::Turn => {
                let mut deg = d.x * 0.5;
                if shift {
                    deg = (deg / 15.0).round() * 15.0;
                }
                (kernels::rotate_by(&drag.lifted, 2, deg), IVec3::ZERO)
            }
        };
        let Some(volume) = loaded.doc.scene.models.get(drag.model) else { return };
        let (writes, cells) = place_moved(volume, &drag.selection, drag.lifted.size(), drag.corner, &after, offset);
        let command = voxel_core::Command::set_voxels(&loaded.doc, drag.model, writes);
        command.apply(&mut loaded.doc);
        drag.preview = Some(command);
        drag.cells = cells;
        self.mesh_dirty = true;
    }

    /// Enter keeps the preview, as one undo step; Esc drops it.
    pub fn finish_transform(&mut self, keep: bool) -> String {
        let Some(drag) = self.transform_drag.take() else { return String::new() };
        let Some(loaded) = self.scene.as_mut() else { return String::new() };
        let Some(preview) = drag.preview else { return "transform: nothing to keep".to_owned() };
        preview.revert(&mut loaded.doc);
        self.mesh_dirty = true;
        if !keep {
            return "transform cancelled".to_owned();
        }
        self.history.end_gesture();
        self.history.exec_alone(&mut loaded.doc, preview);
        let volume = &loaded.doc.scene.models[drag.model];
        let mut next = Selection::new(drag.model);
        next.combine(volume, drag.cells, voxel_core::Combine::Replace);
        self.selection = Some(next);
        "transform kept".to_owned()
    }

    /// Enter and Esc for a pending mouse transform.
    pub fn transform_input(&mut self, ctx: &eframe::egui::Context) {
        if self.transform_drag.is_none() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (enter, escape) = ctx.input(|i| (i.key_pressed(eframe::egui::Key::Enter), i.key_pressed(eframe::egui::Key::Escape)));
        if enter || escape {
            let message = self.finish_transform(enter);
            self.console_log.push(message);
        }
    }

    pub fn transform_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let usage = "transform scale <axes> <factor> | transform rotate <x|y|z> <degrees> | transform drag move|scale|turn|off | transform keep|drop";
        let op: Box<dyn Fn(&Volume) -> Volume> = match words.as_slice() {
            ["scale", axes, f] => {
                let Ok(f) = f.parse::<f32>() else { return usage.to_owned() };
                if f <= 0.0 || !f.is_finite() || axes.is_empty() || !axes.chars().all(|c| "xyz".contains(c)) {
                    return usage.to_owned();
                }
                let s = Vec3::new(
                    if axes.contains('x') { f } else { 1.0 },
                    if axes.contains('y') { f } else { 1.0 },
                    if axes.contains('z') { f } else { 1.0 },
                );
                Box::new(move |v| kernels::scale(v, s))
            }
            ["keep"] | ["drop"] => return self.finish_transform(words[0] == "keep"),
            ["drag", h] => {
                self.transform_tool = match *h {
                    "move" => Some(Handle::Move),
                    "scale" => Some(Handle::Scale),
                    "turn" | "rotate" => Some(Handle::Turn),
                    "off" => None,
                    _ => return "transform drag move|scale|turn|off".to_owned(),
                };
                return match self.transform_tool {
                    Some(_) => "drag in the viewport; Enter keeps, Esc drops".to_owned(),
                    None => "mouse transform off".to_owned(),
                };
            }
            ["rotate", axis, deg] => {
                let a = match *axis {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    _ => return usage.to_owned(),
                };
                let Ok(d) = deg.parse::<f32>() else { return usage.to_owned() };
                Box::new(move |v| kernels::rotate_by(v, a, d))
            }
            _ => return usage.to_owned(),
        };
        let Some(selection) = self.active_selection().cloned() else { return "transform: select voxels first".to_owned() };
        let model = selection.model;
        let Some(loaded) = self.scene.as_mut() else { return "nothing open".to_owned() };
        let Some(volume) = loaded.doc.scene.models.get(model) else { return "no model".to_owned() };
        let Some((lifted, corner)) = lift(volume, &selection) else { return "transform: the selection has no voxels".to_owned() };
        let after = op(&lifted);
        let (writes, cells) = place(volume, &selection, lifted.size(), corner, &after);
        let count = writes.len();
        self.history.end_gesture();
        self.history.set_voxels(&mut loaded.doc, model, writes);
        self.history.end_gesture();
        let volume = &loaded.doc.scene.models[model];
        let mut next = Selection::new(model);
        next.combine(volume, cells, voxel_core::Combine::Replace);
        self.selection = Some(next);
        self.mesh_dirty = true;
        format!("transform: {count} voxels changed")
    }

    pub fn transform_ui(&mut self, ui: &mut eframe::egui::Ui) {
        use crate::layout::{button_row, caption, toggle_row};
        let mut line = None;
        caption(ui, "TRANSFORM");
        if let Some(i) = button_row(ui, &[("×2", "Scale the selection up"), ("×½", "Scale the selection down")]) {
            line = Some(["transform scale xyz 2", "transform scale xyz 0.5"][i]);
        }
        if let Some(i) = button_row(ui, &[("⟲ 15°", "Turn 15° about z"), ("⟳ 15°", "Turn -15° about z")]) {
            line = Some(["transform rotate z 15", "transform rotate z -15"][i]);
        }
        let hint = "Drag in the viewport; Enter keeps, Esc drops";
        let t = self.transform_tool;
        let handles = [Handle::Move, Handle::Scale, Handle::Turn];
        if let Some(i) = toggle_row(ui, &[(t == Some(Handle::Move), "Move", hint), (t == Some(Handle::Scale), "Scale", hint), (t == Some(Handle::Turn), "Turn", hint)]) {
            let h = handles[i];
            self.transform_tool = if t == Some(h) { None } else { Some(h) };
            if self.transform_tool.is_none() {
                let message = self.finish_transform(false);
                self.console_log.push(message);
            }
        }
        if self.transform_drag.is_some()
            && let Some(i) = button_row(ui, &[("Keep", "Enter"), ("Drop", "Esc")])
        {
            let message = self.finish_transform(i == 0);
            self.console_log.push(message);
        }
        if let Some(l) = line {
            let message = self.transform_command(l);
            self.console_log.push(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaling_a_selection_keeps_its_centre() {
        let mut v = Volume::new([16, 16, 16]);
        let mut cells = Vec::new();
        for z in 6..8 {
            for y in 6..8 {
                for x in 6..8 {
                    v.set(x, y, z, 3);
                    cells.push(IVec3::new(x as i32, y as i32, z as i32));
                }
            }
        }
        let mut s = Selection::new(0);
        s.combine(&v, cells, voxel_core::Combine::Replace);
        let (lifted, corner) = lift(&v, &s).unwrap();
        assert_eq!((lifted.size(), corner), ([2, 2, 2], IVec3::splat(6)));
        let big = kernels::scale(&lifted, Vec3::splat(2.0));
        let (writes, placed) = place(&v, &s, lifted.size(), corner, &big);
        assert_eq!(placed.len(), 64);
        // Centred: 6..8 doubled about 7 is 5..9.
        assert!(placed.iter().all(|p| (5..9).contains(&p.x) && (5..9).contains(&p.z)));
        // The old cells are inside the new block, so only new ones change.
        assert_eq!(writes.len(), 64 - 8);
    }
}
