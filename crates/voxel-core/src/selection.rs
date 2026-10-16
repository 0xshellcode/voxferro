//! Selected voxels, and the edits that act on them.

use crate::Volume;
use glam::IVec3;
use std::collections::BTreeSet;

/// A set of cells in one model.
///
/// Not part of the document: like the editor's own selection, changing it is
/// not an undoable edit. An ordered set, so iterating it -- and the commands
/// built from it -- is deterministic.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Selection {
    pub model: usize,
    cells: BTreeSet<[i32; 3]>,
}

/// How a new set of cells combines with the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    Replace,
    Add,
    Subtract,
}

/// Copied voxels, at the coordinates they were copied from.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clipboard {
    pub voxels: Vec<(IVec3, u8)>,
}

impl Selection {
    pub fn new(model: usize) -> Self {
        Self { model, cells: BTreeSet::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// The selected cells, for caches that fingerprint them.
    pub fn cells(&self) -> &BTreeSet<[i32; 3]> {
        &self.cells
    }

    pub fn contains(&self, p: IVec3) -> bool {
        self.cells.contains(&p.to_array())
    }

    pub fn iter(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.cells.iter().map(|&a| IVec3::from_array(a))
    }

    /// Merges `cells` in. Only filled voxels can be selected: a selection
    /// of empty space would copy and move nothing.
    pub fn combine(&mut self, volume: &Volume, cells: impl IntoIterator<Item = IVec3>, how: Combine) {
        if how == Combine::Replace {
            self.cells.clear();
        }
        for p in cells {
            if volume.get_i(p.x, p.y, p.z) == 0 {
                continue;
            }
            match how {
                Combine::Subtract => {
                    self.cells.remove(&p.to_array());
                }
                _ => {
                    self.cells.insert(p.to_array());
                }
            }
        }
    }

    /// Every filled voxel of the model.
    pub fn all(model: usize, volume: &Volume) -> Self {
        let mut s = Self::new(model);
        s.combine(volume, filled_cells(volume), Combine::Add);
        s
    }

    /// Every voxel of one palette index.
    pub fn by_colour(model: usize, volume: &Volume, colour: u8) -> Self {
        let mut s = Self::new(model);
        let cells = filled_cells(volume).filter(|p| volume.get_i(p.x, p.y, p.z) == colour);
        s.combine(volume, cells, Combine::Add);
        s
    }

    /// The filled voxels not selected now.
    pub fn inverted(&self, volume: &Volume) -> Self {
        let mut s = Self::new(self.model);
        let cells = filled_cells(volume).filter(|p| !self.contains(*p));
        s.combine(volume, cells, Combine::Add);
        s
    }

    /// Drops cells that are no longer filled, after an undo or an erase.
    pub fn retain_filled(&mut self, volume: &Volume) {
        self.cells.retain(|a| volume.get_i(a[0], a[1], a[2]) != 0);
    }

    pub fn copy(&self, volume: &Volume) -> Clipboard {
        Clipboard {
            voxels: self.iter().map(|p| (p, volume.get_i(p.x, p.y, p.z))).collect(),
        }
    }

    /// Writes that empty the selected voxels.
    pub fn erase_writes(&self) -> Vec<(IVec3, u8)> {
        self.iter().map(|p| (p, 0)).collect()
    }

    /// Writes that recolour the selected voxels.
    pub fn paint_writes(&self, colour: u8) -> Vec<(IVec3, u8)> {
        self.iter().map(|p| (p, colour)).collect()
    }

    /// Writes that move the selected voxels by `delta`, and the selection
    /// at its new place.
    ///
    /// Clearing comes first and placing second, so where the old and new
    /// positions overlap the moved voxel wins. A voxel moved out of the
    /// model's box is lost, as it would be in the editor.
    pub fn move_writes(&self, volume: &Volume, delta: IVec3) -> (Vec<(IVec3, u8)>, Selection) {
        let clip = self.copy(volume);
        let mut writes = self.erase_writes();
        let mut moved = Selection::new(self.model);
        for &(p, colour) in &clip.voxels {
            let q = p + delta;
            if inside(volume, q) {
                writes.push((q, colour));
                moved.cells.insert(q.to_array());
            }
        }
        (writes, moved)
    }
}

impl Clipboard {
    pub fn is_empty(&self) -> bool {
        self.voxels.is_empty()
    }

    /// Writes that paste at the copied coordinates, and the pasted voxels
    /// as a selection so they can be moved straight away. Cells outside the
    /// target model are dropped.
    pub fn paste_writes(&self, model: usize, volume: &Volume) -> (Vec<(IVec3, u8)>, Selection) {
        let mut pasted = Selection::new(model);
        let writes: Vec<(IVec3, u8)> = self
            .voxels
            .iter()
            .copied()
            .filter(|(p, _)| inside(volume, *p))
            .collect();
        for (p, _) in &writes {
            pasted.cells.insert(p.to_array());
        }
        (writes, pasted)
    }
}

fn inside(volume: &Volume, p: IVec3) -> bool {
    let [sx, sy, sz] = volume.size();
    p.x >= 0 && p.y >= 0 && p.z >= 0 && (p.x as u32) < sx && (p.y as u32) < sy && (p.z as u32) < sz
}

fn filled_cells(volume: &Volume) -> impl Iterator<Item = IVec3> + '_ {
    let [sx, sy, sz] = volume.size();
    (0..sz).flat_map(move |z| {
        (0..sy).flat_map(move |y| {
            (0..sx).filter_map(move |x| {
                (volume.get(x, y, z) != 0).then_some(IVec3::new(x as i32, y as i32, z as i32))
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Volume {
        let mut v = Volume::new([4, 4, 4]);
        v.set(0, 0, 0, 1);
        v.set(1, 0, 0, 2);
        v.set(2, 0, 0, 2);
        v
    }

    #[test]
    fn only_filled_voxels_are_selected() {
        let v = sample();
        let mut s = Selection::new(0);
        s.combine(&v, [IVec3::ZERO, IVec3::new(3, 3, 3)], Combine::Replace);
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn add_and_subtract_combine() {
        let v = sample();
        let mut s = Selection::all(0, &v);
        assert_eq!(s.len(), 3);
        s.combine(&v, [IVec3::ZERO], Combine::Subtract);
        assert_eq!(s.len(), 2);
        assert_eq!(s.inverted(&v).len(), 1);
        assert_eq!(Selection::by_colour(0, &v, 2).len(), 2);
    }

    #[test]
    fn moving_overlapping_cells_keeps_every_voxel() {
        let v = sample();
        let s = Selection::by_colour(0, &v, 2);
        let (writes, moved) = s.move_writes(&v, IVec3::X);
        // Apply in order, as Command::set_voxels does (last write wins).
        let mut w = v.clone();
        for (p, c) in writes {
            w.set(p.x as u32, p.y as u32, p.z as u32, c);
        }
        assert_eq!(w.get(1, 0, 0), 0, "vacated");
        assert_eq!(w.get(2, 0, 0), 2, "overwritten by the moved voxel");
        assert_eq!(w.get(3, 0, 0), 2);
        assert_eq!(moved.len(), 2);
        assert!(moved.contains(IVec3::new(3, 0, 0)));
    }

    #[test]
    fn moving_out_of_the_box_drops_the_voxel() {
        let v = sample();
        let s = Selection::all(0, &v);
        let (_, moved) = s.move_writes(&v, IVec3::new(-1, 0, 0));
        assert_eq!(moved.len(), 2, "the voxel at x = 0 left the model");
    }

    #[test]
    fn paste_lands_where_it_was_copied_and_clips_to_the_target() {
        let v = sample();
        let clip = Selection::all(0, &v).copy(&v);
        let small = Volume::new([2, 2, 2]);
        let (writes, pasted) = clip.paste_writes(1, &small);
        assert_eq!(writes.len(), 2, "x = 2 does not fit a 2-wide model");
        assert_eq!(pasted.model, 1);
    }
}
