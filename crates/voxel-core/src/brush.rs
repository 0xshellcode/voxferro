//! Brushes: which cells a stroke touches, and what it writes there.
//!
//! The editor's tool panel crosses a *mode* (attach, erase, paint) with a
//! *shape* (voxel, box, line, face, fill). Everything here is a pure function
//! from a volume and the stroke's end points to a list of writes; the app
//! turns that list into one undoable command.

use crate::{Face, Hit, Volume};
use glam::IVec3;
use std::collections::VecDeque;

/// What a stroke does to the cells it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushMode {
    /// Fills empty cells; never overwrites a voxel.
    Attach,
    /// Empties voxels.
    Erase,
    /// Recolours voxels; never creates one.
    Paint,
}

/// Which cells a stroke covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrushShape {
    /// One cell per click, or one per cell crossed while dragging.
    Voxel,
    /// A rectangle in the plane of the face the stroke started on.
    Box,
    /// A straight run of cells, in the same plane.
    Line,
    /// The whole flat region of the clicked face: extruded by attach,
    /// removed by erase, recoloured by paint.
    Face,
    /// Flood fill: connected voxels of one colour for erase and paint; the
    /// enclosed empty region of a layer for attach.
    Fill,
}

impl BrushMode {
    pub const ALL: [(BrushMode, &'static str); 3] = [
        (BrushMode::Attach, "attach"),
        (BrushMode::Erase, "erase"),
        (BrushMode::Paint, "paint"),
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, n)| *n == s).map(|(m, _)| *m)
    }

    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(m, _)| *m == self).map_or("", |(_, n)| n)
    }
}

impl BrushShape {
    pub const ALL: [(BrushShape, &'static str); 5] = [
        (BrushShape::Voxel, "voxel"),
        (BrushShape::Box, "box"),
        (BrushShape::Line, "line"),
        (BrushShape::Face, "face"),
        (BrushShape::Fill, "fill"),
    ];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, n)| *n == s).map(|(m, _)| *m)
    }

    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(m, _)| *m == self).map_or("", |(_, n)| n)
    }

    /// Shapes defined by a press and a release, previewed while dragging.
    pub fn is_dragged(self) -> bool {
        matches!(self, BrushShape::Box | BrushShape::Line)
    }
}

/// The cell a stroke is anchored on: the empty cell in front of the hit face
/// when attaching, the hit voxel itself otherwise.
pub fn anchor(mode: BrushMode, hit: &Hit) -> IVec3 {
    match mode {
        BrushMode::Attach => hit.adjacent(),
        _ => hit.voxel,
    }
}

/// Which axis a face is perpendicular to: 0, 1 or 2.
pub fn face_axis(face: Face) -> usize {
    face as usize / 2
}

/// Every cell of the box spanned by two corners, inclusive.
pub fn box_cells(a: IVec3, b: IVec3) -> Vec<IVec3> {
    let (lo, hi) = (a.min(b), a.max(b));
    let mut out = Vec::new();
    for z in lo.z..=hi.z {
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                out.push(IVec3::new(x, y, z));
            }
        }
    }
    out
}

/// A run of cells from `a` to `b`, one per step along the longest axis, so
/// the line has no gaps and no doubled cells.
pub fn line_cells(a: IVec3, b: IVec3) -> Vec<IVec3> {
    let d = b - a;
    let steps = d.x.abs().max(d.y.abs()).max(d.z.abs());
    if steps == 0 {
        return vec![a];
    }
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            a + (d.as_vec3() * t).round().as_ivec3()
        })
        .collect()
}

fn in_bounds(volume: &Volume, p: IVec3) -> bool {
    let [sx, sy, sz] = volume.size();
    p.x >= 0 && p.y >= 0 && p.z >= 0 && (p.x as u32) < sx && (p.y as u32) < sy && (p.z as u32) < sz
}

/// The four in-plane neighbours of a cell, for a plane perpendicular to
/// `axis`.
fn plane_neighbours(p: IVec3, axis: usize) -> [IVec3; 4] {
    let (u, v) = match axis {
        0 => (IVec3::Y, IVec3::Z),
        1 => (IVec3::X, IVec3::Z),
        _ => (IVec3::X, IVec3::Y),
    };
    [p + u, p - u, p + v, p - v]
}

/// Breadth-first flood from `start` over cells `accept` admits.
fn flood(start: IVec3, neighbours: impl Fn(IVec3) -> Vec<IVec3>, accept: impl Fn(IVec3) -> bool) -> Vec<IVec3> {
    if !accept(start) {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::from([start]);
    let mut queue = VecDeque::from([start]);
    let mut out = Vec::new();
    while let Some(p) = queue.pop_front() {
        out.push(p);
        for n in neighbours(p) {
            if accept(n) && seen.insert(n) {
                queue.push_back(n);
            }
        }
    }
    out
}

/// The flat region a face belongs to: voxels of the same colour, connected
/// within the face's plane, whose same face is exposed to the air.
pub fn face_region(volume: &Volume, voxel: IVec3, face: Face) -> Vec<IVec3> {
    let colour = volume.get_i(voxel.x, voxel.y, voxel.z);
    if colour == 0 {
        return Vec::new();
    }
    let o = face.offset();
    let out = IVec3::new(o[0], o[1], o[2]);
    let axis = face_axis(face);
    flood(
        voxel,
        |p| plane_neighbours(p, axis).to_vec(),
        |p| {
            let front = p + out;
            volume.get_i(p.x, p.y, p.z) == colour && volume.get_i(front.x, front.y, front.z) == 0
        },
    )
}

/// Voxels of the same colour connected to `voxel` through shared faces.
pub fn colour_region(volume: &Volume, voxel: IVec3) -> Vec<IVec3> {
    let colour = volume.get_i(voxel.x, voxel.y, voxel.z);
    if colour == 0 {
        return Vec::new();
    }
    flood(
        voxel,
        |p| {
            vec![
                p + IVec3::X,
                p - IVec3::X,
                p + IVec3::Y,
                p - IVec3::Y,
                p + IVec3::Z,
                p - IVec3::Z,
            ]
        },
        |p| in_bounds(volume, p) && volume.get_i(p.x, p.y, p.z) == colour,
    )
}

/// Empty cells connected to `start` within one layer perpendicular to
/// `axis`, bounded by voxels and by the model's box.
pub fn empty_region_in_plane(volume: &Volume, start: IVec3, axis: usize) -> Vec<IVec3> {
    flood(
        start,
        |p| plane_neighbours(p, axis).to_vec(),
        |p| in_bounds(volume, p) && volume.get_i(p.x, p.y, p.z) == 0,
    )
}

/// The cells a stroke covers, before the mode decides what to write.
///
/// `end` is only used by the dragged shapes; the app projects the cursor
/// onto the anchor's plane to get it.
pub fn stroke_cells(volume: &Volume, mode: BrushMode, shape: BrushShape, hit: &Hit, end: IVec3) -> Vec<IVec3> {
    let start = anchor(mode, hit);
    match shape {
        BrushShape::Voxel => vec![start],
        BrushShape::Box => box_cells(start, end),
        BrushShape::Line => line_cells(start, end),
        BrushShape::Face => {
            let region = face_region(volume, hit.voxel, hit.face);
            if mode == BrushMode::Attach {
                let o = hit.face.offset();
                let out = IVec3::new(o[0], o[1], o[2]);
                region.into_iter().map(|p| p + out).collect()
            } else {
                region
            }
        }
        BrushShape::Fill => match mode {
            BrushMode::Attach => empty_region_in_plane(volume, start, face_axis(hit.face)),
            _ => colour_region(volume, hit.voxel),
        },
    }
}

/// Turns covered cells into writes according to the mode.
///
/// Attach only fills empty cells and paint only touches existing voxels, so
/// a box dragged across a model adds around it, or recolours it, without
/// either swallowing or inflating it.
pub fn writes(volume: &Volume, mode: BrushMode, colour: u8, cells: &[IVec3]) -> Vec<(IVec3, u8)> {
    cells
        .iter()
        .filter(|p| in_bounds(volume, **p))
        .filter_map(|&p| {
            let filled = volume.get_i(p.x, p.y, p.z) != 0;
            match mode {
                BrushMode::Attach if !filled => Some((p, colour)),
                BrushMode::Erase if filled => Some((p, 0)),
                BrushMode::Paint if filled => Some((p, colour)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An 8x8 floor at z = 0 of colour 1, with a 2x2 patch of colour 2.
    fn floor() -> Volume {
        let mut v = Volume::new([8, 8, 8]);
        for y in 0..8 {
            for x in 0..8 {
                v.set(x, y, 0, 1);
            }
        }
        for (x, y) in [(3, 3), (4, 3), (3, 4), (4, 4)] {
            v.set(x, y, 0, 2);
        }
        v
    }

    fn top_hit(x: i32, y: i32) -> Hit {
        Hit { voxel: IVec3::new(x, y, 0), face: Face::PosZ }
    }

    #[test]
    fn a_box_is_inclusive_in_any_corner_order() {
        let cells = box_cells(IVec3::new(2, 0, 1), IVec3::new(0, 1, 1));
        assert_eq!(cells.len(), 3 * 2);
        assert!(cells.contains(&IVec3::new(0, 0, 1)) && cells.contains(&IVec3::new(2, 1, 1)));
    }

    #[test]
    fn a_line_has_one_cell_per_step_and_no_gaps() {
        let cells = line_cells(IVec3::ZERO, IVec3::new(6, 3, 0));
        assert_eq!(cells.len(), 7);
        for pair in cells.windows(2) {
            let d = (pair[1] - pair[0]).abs();
            assert!(d.max_element() == 1, "consecutive cells touch: {pair:?}");
        }
        assert_eq!(*cells.last().unwrap(), IVec3::new(6, 3, 0));
    }

    #[test]
    fn attach_box_adds_a_layer_on_top_without_touching_the_floor() {
        let v = floor();
        let cells = stroke_cells(&v, BrushMode::Attach, BrushShape::Box, &top_hit(1, 1), IVec3::new(3, 2, 1));
        let w = writes(&v, BrushMode::Attach, 9, &cells);
        assert_eq!(w.len(), 3 * 2);
        assert!(w.iter().all(|(p, c)| p.z == 1 && *c == 9));
    }

    #[test]
    fn face_paint_recolours_only_the_flat_patch_of_one_colour() {
        let v = floor();
        let cells = stroke_cells(&v, BrushMode::Paint, BrushShape::Face, &top_hit(3, 3), IVec3::ZERO);
        assert_eq!(cells.len(), 4, "the 2x2 patch, not the whole floor");
    }

    #[test]
    fn face_attach_extrudes_the_region() {
        let v = floor();
        let cells = stroke_cells(&v, BrushMode::Attach, BrushShape::Face, &top_hit(0, 0), IVec3::ZERO);
        // The floor of colour 1 minus the patch: 64 - 4.
        assert_eq!(cells.len(), 60);
        assert!(cells.iter().all(|p| p.z == 1));
    }

    #[test]
    fn a_covered_face_is_not_part_of_the_region() {
        let mut v = floor();
        v.set(0, 1, 1, 5); // covers the top of (0, 1, 0)
        let region = face_region(&v, IVec3::new(0, 0, 0), Face::PosZ);
        assert!(!region.contains(&IVec3::new(0, 1, 0)));
        assert_eq!(region.len(), 59);
    }

    #[test]
    fn fill_erase_removes_the_connected_colour_only() {
        let v = floor();
        let cells = stroke_cells(&v, BrushMode::Erase, BrushShape::Fill, &top_hit(4, 4), IVec3::ZERO);
        let w = writes(&v, BrushMode::Erase, 0, &cells);
        assert_eq!(w.len(), 4);
    }

    #[test]
    fn fill_attach_stops_at_walls() {
        // A ring of walls on top of the floor encloses a 2x2 hole.
        let mut v = floor();
        for i in 2..6 {
            for (x, y) in [(i, 2), (i, 5), (2, i), (5, i)] {
                v.set(x, y, 1, 3);
            }
        }
        let cells = stroke_cells(&v, BrushMode::Attach, BrushShape::Fill, &top_hit(3, 3), IVec3::ZERO);
        assert_eq!(cells.len(), 4, "the hole inside the ring, not the layer outside");
    }

    #[test]
    fn modes_never_overreach() {
        let v = floor();
        let cells = [IVec3::new(0, 0, 0), IVec3::new(0, 0, 1), IVec3::new(-1, 0, 0)];
        assert_eq!(writes(&v, BrushMode::Attach, 7, &cells), vec![(IVec3::new(0, 0, 1), 7)]);
        assert_eq!(writes(&v, BrushMode::Erase, 7, &cells), vec![(IVec3::new(0, 0, 0), 0)]);
        assert_eq!(writes(&v, BrushMode::Paint, 7, &cells), vec![(IVec3::new(0, 0, 0), 7)]);
    }

    #[test]
    fn names_round_trip() {
        for (m, n) in BrushMode::ALL {
            assert_eq!(BrushMode::parse(n), Some(m));
            assert_eq!(m.name(), n);
        }
        for (s, n) in BrushShape::ALL {
            assert_eq!(BrushShape::parse(n), Some(s));
        }
    }
}
