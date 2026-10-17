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
    /// A click runs the chosen voxel script over the model, and its result
    /// is attached, erased or painted (the reference editor's shader mode).
    Shader,
    /// A click stamps the pattern, a model kept for the purpose.
    Pattern,
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
    pub const ALL: [(BrushShape, &'static str); 7] = [
        (BrushShape::Voxel, "voxel"),
        (BrushShape::Box, "box"),
        (BrushShape::Line, "line"),
        (BrushShape::Face, "face"),
        (BrushShape::Fill, "fill"),
        (BrushShape::Shader, "shader"),
        (BrushShape::Pattern, "pattern"),
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

/// What a region click takes in (the reference editor's Region options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Every voxel of the clicked colour, anywhere in the model.
    SameColour,
    /// Voxels connected to the clicked one: of its colour only
    /// (`by_colour`) or any; through faces, or (`wide`) through edges and
    /// corners as well -- the reference editor's "4" and "8" neighbours.
    Connected { by_colour: bool, wide: bool },
}

impl Default for Region {
    fn default() -> Self {
        Region::Connected { by_colour: true, wide: false }
    }
}

/// The region `rule` takes in from `voxel`.
pub fn region(volume: &Volume, voxel: IVec3, rule: Region) -> Vec<IVec3> {
    let colour = volume.get_i(voxel.x, voxel.y, voxel.z);
    if colour == 0 {
        return Vec::new();
    }
    match rule {
        Region::SameColour => {
            let [sx, sy, sz] = volume.size();
            let mut out = Vec::new();
            for z in 0..sz {
                for y in 0..sy {
                    for x in 0..sx {
                        if volume.get(x, y, z) == colour {
                            out.push(IVec3::new(x as i32, y as i32, z as i32));
                        }
                    }
                }
            }
            out
        }
        Region::Connected { by_colour, wide } => {
            let mut steps = Vec::new();
            for z in -1..=1 {
                for y in -1..=1 {
                    for x in -1..=1 {
                        let d = IVec3::new(x, y, z);
                        let touching = x.abs() + y.abs() + z.abs();
                        if touching == 1 || wide && touching > 1 {
                            steps.push(d);
                        }
                    }
                }
            }
            flood(
                voxel,
                |p| steps.iter().map(|d| p + *d).collect(),
                |p| {
                    let c = volume.get_i(p.x, p.y, p.z);
                    in_bounds(volume, p) && c != 0 && (!by_colour || c == colour)
                },
            )
        }
    }
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
        // Not cell shapes: the app runs them (see `combine` and
        // `pattern_cells`).
        BrushShape::Shader | BrushShape::Pattern => Vec::new(),
    }
}

/// `cells` stretched across the whole box on each locked axis
/// (the reference editor's Axis mode, which sets the brush's bounds to the model's
/// on those axes). Duplicates are dropped, keeping the first's order.
pub fn through_axes(cells: &[IVec3], size: [u32; 3], axes: [bool; 3]) -> Vec<IVec3> {
    if !axes.iter().any(|&a| a) {
        return cells.to_vec();
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &c in cells {
        let range = |a: usize| if axes[a] { 0..size[a] as i32 } else { c[a]..c[a] + 1 };
        for z in range(2) {
            for y in range(1) {
                for x in range(0) {
                    let p = IVec3::new(x, y, z);
                    if seen.insert(p) {
                        out.push(p);
                    }
                }
            }
        }
    }
    out
}

/// What the shader brush writes: the model `a` and the script's result `t`
/// (both the same size) combined by the mode, as the reference editor's brush
/// kernel does -- attach `a > 0 ? a : t`, erase `t > 0 ? 0 : a`, paint
/// `min(a, t) > 0 ? t : a`.
pub fn combine(volume: &Volume, result: &Volume, mode: BrushMode) -> Vec<(IVec3, u8)> {
    let [sx, sy, sz] = volume.size();
    let mut out = Vec::new();
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let (a, t) = (volume.get(x, y, z), result.get(x, y, z));
                let r = match mode {
                    BrushMode::Attach => if a > 0 { a } else { t },
                    BrushMode::Erase => if t > 0 { 0 } else { a },
                    BrushMode::Paint => if a.min(t) > 0 { t } else { a },
                };
                if r != a {
                    out.push((IVec3::new(x as i32, y as i32, z as i32), r));
                }
            }
        }
    }
    out
}

/// Where a pattern lands for a click, and its colour in each cell: centred
/// on the clicked cell across the face, and stacked out from the face when
/// attaching -- into the model otherwise -- without turning (the pattern
/// keeps the model's axes). `colour`, when given, replaces the pattern's
/// own colours.
pub fn pattern_cells(pattern: &Volume, mode: BrushMode, hit: &Hit, colour: Option<u8>) -> Vec<(IVec3, u8)> {
    let size = pattern.size();
    let axis = face_axis(hit.face);
    let outward = hit.face.offset()[axis] > 0;
    let start = anchor(mode, hit);
    let mut origin = IVec3::ZERO;
    for a in 0..3 {
        origin[a] = start[a] - (size[a] as i32) / 2;
    }
    // Along the normal: the pattern's near side on the anchor, extending
    // away from the surface when attaching and into it otherwise.
    let away = (mode == BrushMode::Attach) == outward;
    origin[axis] = if away { start[axis] } else { start[axis] - size[axis] as i32 + 1 };
    let mut out = Vec::new();
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                let v = pattern.get(x, y, z);
                if v != 0 {
                    out.push((origin + IVec3::new(x as i32, y as i32, z as i32), colour.unwrap_or(v)));
                }
            }
        }
    }
    out
}

/// A pattern's cells turned into writes by the mode, like `writes`, but
/// each cell with its own colour.
pub fn pattern_writes(volume: &Volume, mode: BrushMode, cells: &[(IVec3, u8)]) -> Vec<(IVec3, u8)> {
    cells
        .iter()
        .filter(|(p, _)| in_bounds(volume, *p))
        .filter_map(|&(p, c)| {
            let filled = volume.get_i(p.x, p.y, p.z) != 0;
            match mode {
                BrushMode::Attach if !filled => Some((p, c)),
                BrushMode::Erase if filled => Some((p, 0)),
                BrushMode::Paint if filled => Some((p, c)),
                _ => None,
            }
        })
        .collect()
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

/// `cells` plus their reflections through the model's centre on each
/// enabled axis, and on every combination of them: with x and y on, a cell
/// also lands in the three other quadrants. Cell `c` reflects to
/// `size - 1 - c`, so a model of even size mirrors between its two middle
/// cells and one of odd size about its middle cell. Duplicates are dropped,
/// keeping the first occurrence's order.
pub fn mirrored(cells: &[IVec3], size: [u32; 3], axes: [bool; 3]) -> Vec<IVec3> {
    if !axes.iter().any(|&a| a) {
        return cells.to_vec();
    }
    let mut out = Vec::with_capacity(cells.len() * 8);
    let mut seen = std::collections::HashSet::new();
    for mask in 0..8u32 {
        // Only combinations of enabled axes.
        if (0..3).any(|a| mask & (1 << a) != 0 && !axes[a]) {
            continue;
        }
        for &c in cells {
            let mut m = c;
            for a in 0..3 {
                if mask & (1 << a) != 0 {
                    m[a] = size[a] as i32 - 1 - c[a];
                }
            }
            if seen.insert(m) {
                out.push(m);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirroring_reflects_through_the_centre_on_each_axis() {
        let size = [8, 6, 5];
        let c = IVec3::new(1, 2, 0);
        assert_eq!(mirrored(&[c], size, [false; 3]), vec![c]);
        assert_eq!(mirrored(&[c], size, [true, false, false]), vec![c, IVec3::new(6, 2, 0)]);
        let four = mirrored(&[c], size, [true, true, false]);
        assert_eq!(four.len(), 4);
        assert!(four.contains(&IVec3::new(6, 3, 0)));
        // The middle cell of an odd axis is its own reflection.
        assert_eq!(mirrored(&[IVec3::new(0, 0, 2)], size, [false, false, true]).len(), 1);
    }

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

    #[test]
    fn axis_lock_runs_through_the_box() {
        let c = IVec3::new(1, 2, 3);
        assert_eq!(through_axes(&[c], [4, 5, 6], [false; 3]), vec![c]);
        let column = through_axes(&[c], [4, 5, 6], [false, false, true]);
        assert_eq!(column.len(), 6);
        assert!(column.iter().all(|p| p.x == 1 && p.y == 2));
        // Two layers, each a whole 4x5 plane.
        assert_eq!(through_axes(&[c, IVec3::new(1, 2, 0)], [4, 5, 6], [true, true, false]).len(), 40);
    }

    #[test]
    fn regions_by_rule() {
        let mut v = floor();
        // A colour-2 voxel touching the patch only by an edge, above it.
        v.set(5, 5, 1, 2);
        let n = |rule| region(&v, IVec3::new(3, 3, 0), rule).len();
        assert_eq!(n(Region::default()), 4);
        assert_eq!(n(Region::Connected { by_colour: true, wide: true }), 5);
        assert_eq!(n(Region::SameColour), 5);
        // By geometry the raised voxel joins through the floor cell below it.
        assert_eq!(n(Region::Connected { by_colour: false, wide: false }), 65);
        assert!(region(&v, IVec3::new(0, 0, 5), Region::SameColour).is_empty());
    }

    #[test]
    fn shader_results_combine_by_mode() {
        let v = floor();
        let mut t = Volume::new([8, 8, 8]);
        t.set(0, 0, 0, 9); // over a voxel
        t.set(0, 0, 1, 9); // over empty space
        assert_eq!(combine(&v, &t, BrushMode::Attach), vec![(IVec3::new(0, 0, 1), 9)]);
        assert_eq!(combine(&v, &t, BrushMode::Erase), vec![(IVec3::ZERO, 0)]);
        assert_eq!(combine(&v, &t, BrushMode::Paint), vec![(IVec3::ZERO, 9)]);
    }

    #[test]
    fn patterns_land_on_the_face() {
        // A 3x3x2 pattern, full.
        let mut pattern = Volume::new([3, 3, 2]);
        for z in 0..2 {
            for y in 0..3 {
                for x in 0..3 {
                    pattern.set(x, y, z, 5);
                }
            }
        }
        let hit = top_hit(4, 4);
        let on = pattern_cells(&pattern, BrushMode::Attach, &hit, None);
        let zs: Vec<i32> = on.iter().map(|(p, _)| p.z).collect();
        assert!(zs.iter().all(|&z| z == 1 || z == 2));
        assert!(on.iter().all(|(p, c)| (3..=5).contains(&p.x) && (3..=5).contains(&p.y) && *c == 5));
        // Erasing goes into the surface; the brush colour replaces the
        // pattern's when asked.
        let into = pattern_cells(&pattern, BrushMode::Erase, &hit, Some(7));
        assert!(into.iter().all(|(p, c)| (p.z == 0 || p.z == -1) && *c == 7));
        // From below, attaching stacks downward.
        let under = Hit { voxel: IVec3::new(4, 4, 3), face: Face::NegZ };
        assert!(pattern_cells(&pattern, BrushMode::Attach, &under, None).iter().all(|(p, _)| p.z == 1 || p.z == 2));
        let writes = pattern_writes(&floor(), BrushMode::Attach, &on);
        assert_eq!(writes.len(), 18);
        assert_eq!(pattern_writes(&floor(), BrushMode::Paint, &into).len(), 9);
    }
}
