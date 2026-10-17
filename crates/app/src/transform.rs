//! The Transform tool's scale and rotate, on the selected voxels.
//!
//! the reference editor's Transform options (`m_brush.ui`) are move/loop, scale, wrap
//! and rotate, done with the mouse and committed with Enter. Moving is the
//! editor's `move` (and the arrow keys); here are scale and rotate, as
//! commands and panel buttons:
//!
//! - `transform scale <axes> <factor>`: `transform scale xyz 2`;
//! - `transform rotate <axis> <degrees>`: any angle.
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
    let mut writes: BTreeMap<[i32; 3], u8> = BTreeMap::new();
    for p in selection.iter().filter(|p| volume.get_i(p.x, p.y, p.z) != 0) {
        writes.insert(p.to_array(), 0);
    }
    let centre = corner.as_vec3() + Vec3::from_array(before_size.map(|s| s as f32)) * 0.5;
    let new_corner = (centre - Vec3::from_array(after.size().map(|s| s as f32)) * 0.5).round().as_ivec3();
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

impl App {
    pub fn transform_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let usage = "transform scale <axes> <factor> | transform rotate <x|y|z> <degrees>";
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
        let mut line = None;
        ui.horizontal_wrapped(|ui| {
            ui.label("Transform");
            for (label, l, hint) in [
                ("×2", "transform scale xyz 2", "Scale the selection up"),
                ("×½", "transform scale xyz 0.5", "Scale the selection down"),
                ("⟲ 15°", "transform rotate z 15", "Turn the selection 15° about z"),
                ("⟳ 15°", "transform rotate z -15", "Turn the selection -15° about z"),
            ] {
                if ui.small_button(label).on_hover_text(hint).clicked() {
                    line = Some(l);
                }
            }
        });
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
