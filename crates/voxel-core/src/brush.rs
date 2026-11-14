//! Brushes: which cells a stroke touches, and what it writes there.
//!
//! The editor's tool panel crosses a *mode* (attach, erase, paint) with a
//! *shape* (voxel, box, line, face, fill). Everything here is a pure function
//! from a volume and the stroke's end points to a list of writes; the app
//! turns that list into one undoable command.

use crate::{Face, Hit, Volume};
use glam::IVec3;

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
    /// A square in the face's plane, dragged out from its centre.
    Square,
    /// A circle in the face's plane, dragged out from its centre.
    Circle,
    /// A click runs the chosen voxel script over the model, and its result
    /// is attached, erased or painted.
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
    pub const ALL: [(BrushShape, &'static str); 9] = [
        (BrushShape::Voxel, "voxel"),
        (BrushShape::Box, "box"),
        (BrushShape::Line, "line"),
        (BrushShape::Square, "square"),
        (BrushShape::Circle, "circle"),
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
        matches!(self, BrushShape::Box | BrushShape::Line | BrushShape::Square | BrushShape::Circle)
    }
}

/// The brush options of the Brush panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BrushOptions {
    /// The voxel brush's size, 1 to 64 ("Voxel Size").
    pub size: u32,
    /// A round tip instead of a square one ("Cube/Sphere Shape").
    pub sphere: bool,
    /// A tip flat in the face's plane instead of a solid ("2D/3D Shape").
    pub flat: bool,
    /// Square and circle filled, not just their outline ("Fill Center").
    pub fill_centre: bool,
    /// Square and circle of even size, centred on a corner ("Even").
    pub even: bool,
    /// The line laid onto the surface below it ("Project Line").
    pub project_line: bool,
    /// The face brush's region: same colour only, or any ("Color/Geo").
    pub face_by_colour: bool,
    /// ... through edges as well as sides ("4/8 Neighbor").
    pub face_wide: bool,
    /// Extruding keeps each voxel's colour instead of the brush's ("Use
    /// Voxel Color").
    pub face_keep_colour: bool,
}

impl Default for BrushOptions {
    fn default() -> Self {
        Self {
            size: 1,
            sphere: false,
            flat: false,
            fill_centre: true,
            even: false,
            project_line: false,
            face_by_colour: true,
            face_wide: false,
            face_keep_colour: false,
        }
    }
}

/// The two axes of a face's plane.
fn plane_axes(axis: usize) -> (usize, usize) {
    match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

/// The voxel brush's tip at `centre`: `size` cells across, square or
/// round, flat in the plane across `axis` or solid. A solid tip that
/// `outward` is nonzero for (attaching) grows from the surface outward along
/// that sign of `axis`; otherwise it is centred.
pub fn tip_cells(centre: IVec3, axis: usize, outward: i32, o: &BrushOptions) -> Vec<IVec3> {
    let n = o.size.clamp(1, 64) as i32;
    if n == 1 {
        return vec![centre];
    }
    let lo = -((n - 1) / 2);
    let hi = lo + n - 1;
    let (u, v) = plane_axes(axis);
    let along: Vec<i32> = match (o.flat, outward.signum()) {
        (true, _) => vec![0],
        (false, 0) => (lo..=hi).collect(),
        (false, s) => (0..n).map(|k| k * s).collect(),
    };
    // The tip's centre in offsets, for the round test.
    let mid = (lo + hi) as f32 * 0.5;
    let mid_along = if along.len() == 1 { 0.0 } else { (along[0] + along[along.len() - 1]) as f32 * 0.5 };
    let r2 = (n as f32 * 0.5).powi(2);
    let mut out = Vec::new();
    for &a in &along {
        for j in lo..=hi {
            for i in lo..=hi {
                if o.sphere {
                    let (di, dj, da) = (i as f32 - mid, j as f32 - mid, a as f32 - mid_along);
                    if di * di + dj * dj + da * da > r2 {
                        continue;
                    }
                }
                let mut p = centre;
                p[u] += i;
                p[v] += j;
                p[axis] += a;
                out.push(p);
            }
        }
    }
    out
}

/// A square or circle in the plane across `axis` through `centre`, out to
/// `end`: its radius is how far the drag went (the larger of the two plane
/// distances for a square, the straight distance for a circle). Even
/// shapes are centred on `centre`'s far corner; without `fill_centre` only
/// the outline is kept.
pub fn centre_cells(centre: IVec3, end: IVec3, axis: usize, round: bool, o: &BrushOptions) -> Vec<IVec3> {
    let (u, v) = plane_axes(axis);
    let (du, dv) = ((end[u] - centre[u]) as f32, (end[v] - centre[v]) as f32);
    let r = if round { (du * du + dv * dv).sqrt().round() } else { du.abs().max(dv.abs()) };
    let shift = if o.even { 0.5 } else { 0.0 };
    let reach = r as i32 + 1;
    // Inside: by the larger distance for a square, the straight one for a
    // circle; the half added to a circle's radius rounds its outline.
    let inside = |i: f32, j: f32, limit: f32| {
        if round { i * i + j * j <= (limit + 0.5) * (limit + 0.5) } else { i.abs().max(j.abs()) <= limit }
    };
    let mut out = Vec::new();
    for j in -reach..=reach {
        for i in -reach..=reach {
            let (fi, fj) = (i as f32 - shift, j as f32 - shift);
            // Even: one wider than odd, so the centre falls on a corner.
            let limit = if o.even { r + 0.5 } else { r };
            if !inside(fi, fj, limit) {
                continue;
            }
            if !o.fill_centre && inside(fi, fj, limit - 1.0) {
                continue;
            }
            let mut p = centre;
            p[u] += i;
            p[v] += j;
            out.push(p);
        }
    }
    out
}

/// A line's cells laid onto the surface, along `axis` toward the inside of
/// the face it was drawn on (`outward` is the face's sign): each lands on
/// the first voxel it meets coming in from outside the box -- in front of
/// it when attaching, on it otherwise. Cells that meet nothing land on the
/// box's far side.
pub fn project_onto_surface(volume: &Volume, cells: &[IVec3], axis: usize, outward: i32, mode: BrushMode) -> Vec<IVec3> {
    let size = volume.size()[axis] as i32;
    let step = -outward.signum();
    let start = if step < 0 { size - 1 } else { 0 };
    cells
        .iter()
        .map(|&c| {
            let mut p = c;
            let mut k = start;
            while (0..size).contains(&k) {
                p[axis] = k;
                if volume.get_i(p.x, p.y, p.z) != 0 {
                    if mode == BrushMode::Attach {
                        p[axis] = k - step;
                    }
                    return p;
                }
                k += step;
            }
            // Nothing below: the far side of the box.
            p[axis] = start + step * (size - 1);
            if mode == BrushMode::Attach { p } else { c }
        })
        .collect()
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

/// The six face neighbours' offsets.
const FACE_STEPS: [IVec3; 6] = [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z];

/// Breadth-first flood from `start` over the cells of a `size` box that
/// `accept` admits, stepping by `steps`. Visited cells are marked in a bit
/// per cell of the box, and the output doubles as the queue: a whole 256³
/// model floods in a fraction of a second.
fn flood(size: [u32; 3], start: IVec3, steps: &[IVec3], accept: impl Fn(IVec3) -> bool) -> Vec<IVec3> {
    let [sx, sy, sz] = size.map(|s| s as i32);
    let inside = |p: IVec3| p.x >= 0 && p.y >= 0 && p.z >= 0 && p.x < sx && p.y < sy && p.z < sz;
    if !inside(start) || !accept(start) {
        return Vec::new();
    }
    let index = |p: IVec3| (p.x + sx * (p.y + sy * p.z)) as usize;
    let mut seen = vec![0u64; (size.iter().map(|&s| s as usize).product::<usize>()).div_ceil(64)];
    let mut mark = |p: IVec3| {
        let i = index(p);
        let fresh = seen[i / 64] & (1 << (i % 64)) == 0;
        seen[i / 64] |= 1 << (i % 64);
        fresh
    };
    mark(start);
    let mut out = vec![start];
    let mut head = 0;
    while head < out.len() {
        let p = out[head];
        head += 1;
        for d in steps {
            let q = p + *d;
            if inside(q) && accept(q) && mark(q) {
                out.push(q);
            }
        }
    }
    out
}

/// The flat region a face belongs to, by the face options: voxels of the
/// clicked colour (or any), connected within the face's plane through
/// sides (or edges too), whose same face is exposed to the air.
pub fn face_region_with(volume: &Volume, voxel: IVec3, face: Face, by_colour: bool, wide: bool) -> Vec<IVec3> {
    let colour = volume.get_i(voxel.x, voxel.y, voxel.z);
    if colour == 0 {
        return Vec::new();
    }
    let o = face.offset();
    let out = IVec3::new(o[0], o[1], o[2]);
    let axis = face_axis(face);
    let (u, v) = plane_axes(axis);
    let steps: Vec<IVec3> = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)]
        .into_iter()
        .filter(|(du, dv)| wide || *du == 0 || *dv == 0)
        .map(|(du, dv)| {
            let mut q = IVec3::ZERO;
            q[u] = du;
            q[v] = dv;
            q
        })
        .collect();
    flood(
        volume.size(),
        voxel,
        &steps,
        |p| {
            let c = volume.get_i(p.x, p.y, p.z);
            let front = p + out;
            c != 0 && (!by_colour || c == colour) && volume.get_i(front.x, front.y, front.z) == 0
        },
    )
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
        volume.size(),
        voxel,
        &plane_neighbours(IVec3::ZERO, axis),
        |p| {
            let front = p + out;
            volume.get_i(p.x, p.y, p.z) == colour && volume.get_i(front.x, front.y, front.z) == 0
        },
    )
}

/// What a region click takes in (the Region options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Every voxel of the clicked colour, anywhere in the model.
    SameColour,
    /// Voxels connected to the clicked one: of its colour only
    /// (`by_colour`) or any; through faces, or (`wide`) through edges and
    /// corners as well -- the "4" and "8" neighbours.
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
            flood(volume.size(), voxel, &steps, |p| {
                let c = volume.get_i(p.x, p.y, p.z);
                c != 0 && (!by_colour || c == colour)
            })
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
        volume.size(),
        voxel,
        &FACE_STEPS,
        |p| volume.get_i(p.x, p.y, p.z) == colour,
    )
}

/// Empty cells connected to `start` within one layer perpendicular to
/// `axis`, bounded by voxels and by the model's box.
pub fn empty_region_in_plane(volume: &Volume, start: IVec3, axis: usize) -> Vec<IVec3> {
    flood(volume.size(), start, &plane_neighbours(IVec3::ZERO, axis), |p| volume.get_i(p.x, p.y, p.z) == 0)
}

/// The cells a stroke covers, before the mode decides what to write.
///
/// `end` is only used by the dragged shapes; the app projects the cursor
/// onto the anchor's plane to get it.
pub fn stroke_cells(volume: &Volume, mode: BrushMode, shape: BrushShape, hit: &Hit, end: IVec3, o: &BrushOptions) -> Vec<IVec3> {
    let start = anchor(mode, hit);
    let axis = face_axis(hit.face);
    let outward = hit.face.offset()[axis];
    match shape {
        BrushShape::Voxel => tip_cells(start, axis, if mode == BrushMode::Attach { outward } else { 0 }, o),
        BrushShape::Box => box_cells(start, end),
        BrushShape::Line if o.project_line => project_onto_surface(volume, &line_cells(start, end), axis, outward, mode),
        BrushShape::Line => line_cells(start, end),
        BrushShape::Square => centre_cells(start, end, axis, false, o),
        BrushShape::Circle => centre_cells(start, end, axis, true, o),
        BrushShape::Face => {
            let region = face_region_with(volume, hit.voxel, hit.face, o.face_by_colour, o.face_wide);
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
/// (the Axis mode, which sets the brush's bounds to the model's on those
/// axes). Duplicates are dropped, keeping the first's order.
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
/// (both the same size) combined by the mode -- attach `a > 0 ? a : t`, erase `t > 0 ? 0 : a`, paint
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
        let cells = stroke_cells(&v, BrushMode::Attach, BrushShape::Box, &top_hit(1, 1), IVec3::new(3, 2, 1), &BrushOptions::default());
        let w = writes(&v, BrushMode::Attach, 9, &cells);
        assert_eq!(w.len(), 3 * 2);
        assert!(w.iter().all(|(p, c)| p.z == 1 && *c == 9));
    }

    #[test]
    fn face_paint_recolours_only_the_flat_patch_of_one_colour() {
        let v = floor();
        let cells = stroke_cells(&v, BrushMode::Paint, BrushShape::Face, &top_hit(3, 3), IVec3::ZERO, &BrushOptions::default());
        assert_eq!(cells.len(), 4, "the 2x2 patch, not the whole floor");
    }

    #[test]
    fn face_attach_extrudes_the_region() {
        let v = floor();
        let cells = stroke_cells(&v, BrushMode::Attach, BrushShape::Face, &top_hit(0, 0), IVec3::ZERO, &BrushOptions::default());
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
        let cells = stroke_cells(&v, BrushMode::Erase, BrushShape::Fill, &top_hit(4, 4), IVec3::ZERO, &BrushOptions::default());
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
        let cells = stroke_cells(&v, BrushMode::Attach, BrushShape::Fill, &top_hit(3, 3), IVec3::ZERO, &BrushOptions::default());
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

    #[test]
    fn tips_by_size_shape_and_depth() {
        let o = |size, sphere, flat| BrushOptions { size, sphere, flat, ..Default::default() };
        let c = IVec3::new(10, 10, 10);
        assert_eq!(tip_cells(c, 2, 1, &o(1, false, false)), vec![c]);
        assert_eq!(tip_cells(c, 2, 0, &o(3, false, false)).len(), 27);
        assert_eq!(tip_cells(c, 2, 0, &o(3, false, true)).len(), 9);
        // Attaching grows outward from the surface: z from 10 up.
        let up = tip_cells(c, 2, 1, &o(3, false, false));
        assert!(up.iter().all(|p| (10..=12).contains(&p.z)));
        let down = tip_cells(c, 2, -1, &o(3, false, false));
        assert!(down.iter().all(|p| (8..=10).contains(&p.z)));
        // A round tip is smaller than its cube, and even sizes work.
        let ball = tip_cells(c, 2, 0, &o(5, true, false)).len();
        assert!(ball < 125 && ball > 50, "{ball}");
        assert_eq!(tip_cells(c, 2, 0, &o(4, false, true)).len(), 16);
    }

    #[test]
    fn squares_and_circles_from_the_centre() {
        let filled = BrushOptions::default();
        let c = IVec3::new(5, 5, 0);
        let sq = centre_cells(c, IVec3::new(7, 6, 0), 2, false, &filled);
        assert_eq!(sq.len(), 25);
        let even = BrushOptions { even: true, ..filled };
        assert_eq!(centre_cells(c, IVec3::new(7, 6, 0), 2, false, &even).len(), 36);
        let ring = BrushOptions { fill_centre: false, ..filled };
        assert_eq!(centre_cells(c, IVec3::new(7, 6, 0), 2, false, &ring).len(), 16);
        let disc = centre_cells(c, IVec3::new(8, 5, 0), 2, true, &filled);
        assert!(disc.len() > 25 && disc.len() < 49, "{}", disc.len());
        assert!(disc.contains(&IVec3::new(8, 5, 0)) && !disc.contains(&IVec3::new(8, 8, 0)));
        assert!(centre_cells(c, c, 2, true, &filled) == vec![c]);
    }

    #[test]
    fn lines_land_on_the_surface() {
        // A floor at z = 0 with a block at (4, 4, 1..=3).
        let mut v = floor();
        for z in 1..4 {
            v.set(4, 4, z, 1);
        }
        let line = [IVec3::new(3, 4, 1), IVec3::new(4, 4, 1), IVec3::new(5, 4, 1)];
        let on = project_onto_surface(&v, &line, 2, 1, BrushMode::Attach);
        assert_eq!(on, vec![IVec3::new(3, 4, 1), IVec3::new(4, 4, 4), IVec3::new(5, 4, 1)]);
        let paint = project_onto_surface(&v, &line, 2, 1, BrushMode::Paint);
        assert_eq!(paint[1], IVec3::new(4, 4, 3));
    }

    #[test]
    fn face_options_widen_the_region() {
        let v = floor();
        let hit = top_hit(0, 0);
        assert_eq!(face_region_with(&v, hit.voxel, hit.face, true, false).len(), 60);
        assert_eq!(face_region_with(&v, hit.voxel, hit.face, false, false).len(), 64);
    }
}
