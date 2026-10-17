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
//! And with the mouse, as the reference editor's tool works: choose Move, Scale,
//! Turn or Wrap, then drag in the viewport -- right and left (and for Move
//! and Wrap, up and down) -- watching a preview; Enter keeps it as one undo
//! step, Esc drops it. Shift snaps a turn to 15°.
//!
//! Wrap is the reference editor's (`the reference renderer`, hint "Wrap [N] : Commit [Enter] :
//! Cancel [Esc] : Two-sided [SHIFT]"): the selection's box is stretched and
//! the selection repeated to fill it ([`kernels::wrap`]), copies apart by
//! the Wrap Spacing, every other one reflected on the axes whose Mirror
//! toggle is on; a box pulled in smaller crops it. the reference editor drags a face
//! of a box gizmo; here a drag moves the face the screen's right (or up)
//! points to, a cell per 12 points, and pulls it in going back. Shift moves
//! the opposite face too. `transform wrap <x|-x|y|-y|z|-z> <cells> [two]`
//! does the same from the console, `transform spacing <x> <y> <z>` sets the
//! gaps.
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
    let centre = corner.as_vec3() + Vec3::from_array(before_size.map(|s| s as f32)) * 0.5;
    let new_corner = (centre - Vec3::from_array(after.size().map(|s| s as f32)) * 0.5).round().as_ivec3() + offset;
    place_at(volume, selection, after, new_corner)
}

/// The writes that replace the lifted voxels by `after`, its corner at
/// `new_corner`, and the cells `after` fills.
pub fn place_at(volume: &Volume, selection: &Selection, after: &Volume, new_corner: IVec3) -> (Vec<(IVec3, u8)>, Vec<IVec3>) {
    let mut writes: BTreeMap<[i32; 3], u8> = BTreeMap::new();
    for p in selection.iter().filter(|p| volume.get_i(p.x, p.y, p.z) != 0) {
        writes.insert(p.to_array(), 0);
    }
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
    Wrap,
}

/// A wrap's box about the lifted one: its low and high corners, in cells
/// from the lifted box's corner (high exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapBox {
    pub lo: IVec3,
    pub hi: IVec3,
}

impl WrapBox {
    pub fn of(size: [u32; 3]) -> Self {
        Self { lo: IVec3::ZERO, hi: IVec3::from_array(size.map(|s| s as i32)) }
    }

    /// Moves the face `face` points to out by `cells` (in, if negative);
    /// with `two_sided`, the opposite face too. The box keeps a cell.
    pub fn pull(&mut self, face: IVec3, cells: i32, two_sided: bool) {
        for a in 0..3 {
            let (high, low) = match face[a] {
                1 => (true, two_sided),
                -1 => (two_sided, true),
                _ => continue,
            };
            if high {
                self.hi[a] += cells;
            }
            if low {
                self.lo[a] -= cells;
            }
            if self.hi[a] - self.lo[a] < 1 {
                // Pulled through: keep a cell on the side that did not move.
                if high && !low {
                    self.hi[a] = self.lo[a] + 1;
                } else if low && !high {
                    self.lo[a] = self.hi[a] - 1;
                } else {
                    let mid = (self.lo[a] + self.hi[a]).div_euclid(2);
                    (self.lo[a], self.hi[a]) = (mid, mid + 1);
                }
            }
        }
    }

    pub fn size(&self) -> [u32; 3] {
        (self.hi - self.lo).max(IVec3::ONE).as_uvec3().to_array()
    }
}

/// `lifted` wrapped into `wrap`, and where its corner goes in the model.
fn wrapped(lifted: &Volume, corner: IVec3, wrap: WrapBox, spacing: [u32; 3], mirror: [bool; 3]) -> (Volume, IVec3) {
    (kernels::wrap(lifted, wrap.lo, wrap.size(), spacing, mirror), corner + wrap.lo)
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
            Handle::Wrap => {
                let mut b = WrapBox::of(drag.lifted.size());
                b.pull(dominant(basis.x_axis), (d.x / 12.0).round() as i32, shift);
                b.pull(dominant(basis.y_axis), (-d.y / 12.0).round() as i32, shift);
                let (after, at) = wrapped(&drag.lifted, drag.corner, b, self.wrap_spacing, self.mirror);
                let Some(volume) = loaded.doc.scene.models.get(drag.model) else { return };
                let (writes, cells) = place_at(volume, &drag.selection, &after, at);
                let command = voxel_core::Command::set_voxels(&loaded.doc, drag.model, writes);
                command.apply(&mut loaded.doc);
                drag.preview = Some(command);
                drag.cells = cells;
                self.mesh_dirty = true;
                return;
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
        let usage = "transform scale <axes> <factor> | transform rotate <x|y|z> <degrees> | transform wrap <x|-x|y|-y|z|-z> <cells> [two] | transform spacing <x> <y> <z> | transform drag move|scale|turn|wrap|off | transform keep|drop";
        let mut wrap_box = None;
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
            ["spacing", x, y, z] => {
                return match (x.parse::<u32>(), y.parse::<u32>(), z.parse::<u32>()) {
                    (Ok(x), Ok(y), Ok(z)) if x.max(y).max(z) <= 256 => {
                        self.wrap_spacing = [x, y, z];
                        format!("wrap spacing {x} {y} {z}")
                    }
                    _ => "transform spacing <x> <y> <z>, each 0 to 256".to_owned(),
                };
            }
            ["wrap", face, cells, rest @ ..] => {
                let face = match *face {
                    "x" | "+x" => IVec3::X,
                    "-x" => IVec3::NEG_X,
                    "y" | "+y" => IVec3::Y,
                    "-y" => IVec3::NEG_Y,
                    "z" | "+z" => IVec3::Z,
                    "-z" => IVec3::NEG_Z,
                    _ => return usage.to_owned(),
                };
                let (Ok(cells), two) = (cells.parse::<i32>(), rest == ["two"]) else { return usage.to_owned() };
                if !(rest.is_empty() || two) || cells.abs() > 256 {
                    return usage.to_owned();
                }
                wrap_box = Some((face, cells, two));
                Box::new(|v: &Volume| v.clone())
            }
            ["drag", h] => {
                self.transform_tool = match *h {
                    "move" => Some(Handle::Move),
                    "scale" => Some(Handle::Scale),
                    "turn" | "rotate" => Some(Handle::Turn),
                    "wrap" => Some(Handle::Wrap),
                    "off" => None,
                    _ => return "transform drag move|scale|turn|wrap|off".to_owned(),
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
        let (writes, cells) = match wrap_box {
            Some((face, n, two)) => {
                let mut b = WrapBox::of(lifted.size());
                b.pull(face, n, two);
                let (after, at) = wrapped(&lifted, corner, b, self.wrap_spacing, self.mirror);
                place_at(volume, &selection, &after, at)
            }
            None => place(volume, &selection, lifted.size(), corner, &op(&lifted)),
        };
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
        let wrap_hint = "Wrap: drag to stretch the box and repeat the selection in it; Shift both sides; Enter keeps, Esc drops";
        let t = self.transform_tool;
        let handles = [Handle::Move, Handle::Scale, Handle::Turn, Handle::Wrap];
        if let Some(i) = toggle_row(
            ui,
            &[
                (t == Some(Handle::Move), "Move", hint),
                (t == Some(Handle::Scale), "Scale", hint),
                (t == Some(Handle::Turn), "Turn", hint),
                (t == Some(Handle::Wrap), "Wrap", wrap_hint),
            ],
        ) {
            let h = handles[i];
            self.transform_tool = if t == Some(h) { None } else { Some(h) };
            if self.transform_tool.is_none() {
                let message = self.finish_transform(false);
                self.console_log.push(message);
            }
        }
        if t == Some(Handle::Wrap) {
            // the reference editor's Wrap Spacing: the gap between copies, per axis.
            ui.horizontal(|ui| {
                ui.label("Gap").on_hover_text("Wrap spacing: empty cells between copies, x y z");
                let gap = ui.spacing().item_spacing.x;
                let w = ((ui.available_width() - 2.0 * gap) / 3.0).max(16.0);
                // A drag value is at least the interact width; narrower here.
                ui.spacing_mut().interact_size.x = w;
                for s in &mut self.wrap_spacing {
                    ui.add_sized([w, 20.0], eframe::egui::DragValue::new(s).range(0..=256).speed(0.1));
                }
            });
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
    fn a_wrap_box_pulls_one_face_or_both_and_keeps_a_cell() {
        let mut b = WrapBox::of([4, 2, 2]);
        b.pull(IVec3::X, 6, false);
        assert_eq!((b.lo, b.hi), (IVec3::ZERO, IVec3::new(10, 2, 2)));
        b.pull(IVec3::NEG_Y, 3, true);
        assert_eq!((b.lo.y, b.hi.y), (-3, 5));
        // Pulled in past the far face: one cell is left.
        let mut c = WrapBox::of([4, 2, 2]);
        c.pull(IVec3::NEG_X, -9, false);
        assert_eq!((c.lo.x, c.hi.x), (3, 4));
        assert_eq!(c.size(), [1, 2, 2]);
    }

    #[test]
    fn wrapping_a_selection_repeats_it_beside_itself() {
        let mut v = Volume::new([16, 4, 4]);
        v.set(2, 1, 1, 5);
        v.set(3, 1, 1, 6);
        let mut s = Selection::new(0);
        s.combine(&v, vec![IVec3::new(2, 1, 1), IVec3::new(3, 1, 1)], voxel_core::Combine::Replace);
        let (lifted, corner) = lift(&v, &s).unwrap();
        let mut b = WrapBox::of(lifted.size());
        b.pull(IVec3::X, 4, false);
        let (after, at) = wrapped(&lifted, corner, b, [1, 0, 0], [false; 3]);
        let (writes, cells) = place_at(&v, &s, &after, at);
        assert_eq!(cells.len(), 4, "the source and a copy: 5 6 _ 5 6 _");
        assert!(cells.contains(&IVec3::new(5, 1, 1)) && cells.contains(&IVec3::new(6, 1, 1)));
        // The source's own cells stay as they are; only the copy is new.
        assert_eq!(writes.len(), 2);
    }

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
