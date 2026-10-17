//! The Edit panel's operations, as the reference editor defines them.
//!
//! the reference editor runs most of these as small GLSL kernels: a `map(dst_p)`
//! evaluated at the centre of every cell of the result (`dst_p` is
//! `(x + 0.5, y + 0.5, z + 0.5)`), reading the model through `voxel(p)`,
//! which floors `p` and returns 0 outside the box. The kernels' text is in
//! its executable; each function here names the one it follows, and keeps
//! its arithmetic, so results match cell for cell.
//!
//! Hull and flood are not kernels there and are written from what their
//! buttons promise.

use crate::Volume;
use glam::{Mat3, Vec3};

/// Evaluates `map` at every cell centre of a `size` box.
fn map_volume(size: [u32; 3], map: impl Fn(Vec3) -> u8) -> Volume {
    let mut out = Volume::new(size);
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                let v = map(Vec3::new(x as f32, y as f32, z as f32) + 0.5);
                if v != 0 {
                    out.set(x, y, z, v);
                }
            }
        }
    }
    out
}

/// The kernels' `voxel()`: the cell `p` falls in, 0 outside the box.
fn voxel(volume: &Volume, p: Vec3) -> u8 {
    let c = p.floor();
    let [sx, sy, sz] = volume.size();
    if c.x < 0.0 || c.y < 0.0 || c.z < 0.0 || c.x >= sx as f32 || c.y >= sy as f32 || c.z >= sz as f32 {
        return 0;
    }
    volume.get(c.x as u32, c.y as u32, c.z as u32)
}

fn size_vec(volume: &Volume) -> Vec3 {
    Vec3::from_array(volume.size().map(|s| s as f32))
}

fn unit(axis: usize) -> Vec3 {
    Vec3::AXES[axis]
}

/// Clamps a computed box size to the format's 1..=256.
fn clamp_size(v: Vec3) -> [u32; 3] {
    v.to_array().map(|s| (s.round() as i64).clamp(1, 256) as u32)
}

/// *Full*: every cell `color`.
pub fn full(volume: &Volume, color: u8) -> Volume {
    map_volume(volume.size(), |_| color)
}

/// *Fill*: every voxel repainted `color`: `voxel > 0 ? color : 0`.
pub fn recolour(volume: &Volume, color: u8) -> Volume {
    map_volume(volume.size(), |p| if voxel(volume, p) > 0 { color } else { 0 })
}

/// *Reverse*: `voxel > 0 ? 0 : color`.
pub fn reverse(volume: &Volume, color: u8) -> Volume {
    map_volume(volume.size(), |p| if voxel(volume, p) > 0 { 0 } else { color })
}

/// *Loop*: moves the contents by `offset` cells along `axis`, wrapping at
/// the edges: `voxel(mod(dst_p - loop, size))`.
pub fn shift_wrap(volume: &Volume, axis: usize, offset: i32) -> Volume {
    let size = size_vec(volume);
    let shift = unit(axis) * offset as f32;
    map_volume(volume.size(), |p| {
        let q = p - shift;
        voxel(volume, ((q % size) + size) % size)
    })
}

/// *Scale*: the box and its contents scaled by `scale` on each axis,
/// sampling `voxel((dst_p) * inv_scale)`.
pub fn scale(volume: &Volume, scale: Vec3) -> Volume {
    let size = clamp_size(size_vec(volume) * scale);
    let inv = scale.recip();
    map_volume(size, |p| voxel(volume, p * inv))
}

/// *2X*: twice the size (each axis capped at 256).
pub fn double(volume: &Volume) -> Volume {
    let size = clamp_size(size_vec(volume) * 2.0);
    map_volume(size, |p| voxel(volume, p * 0.5))
}

/// *1/2*: half the size. Each new cell takes the first filled cell of its
/// 2×2×2 block, in the kernel's order (x fastest, then y, then z).
pub fn halve(volume: &Volume) -> Volume {
    let size = volume.size().map(|s| s.div_ceil(2));
    map_volume(size, |p| {
        let base = p.floor() * 2.0 + 0.5;
        for dz in 0..2 {
            for dy in 0..2 {
                for dx in 0..2 {
                    let c = voxel(volume, base + Vec3::new(dx as f32, dy as f32, dz as f32));
                    if c > 0 {
                        return c;
                    }
                }
            }
        }
        0
    })
}

/// The two axes a shear along `axis` moves, in order: the other two,
/// ascending.
pub fn other_axes(axis: usize) -> [usize; 2] {
    match axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    }
}

/// *Shear*: each layer across `axis` slides by its depth times `amounts`
/// on the other two axes (`other_axes`), in the same box:
/// `depth = dot(p - 0.5, axis); voxel(p - depth * shear)`.
pub fn shear(volume: &Volume, axis: usize, amounts: [f32; 2]) -> Volume {
    let [a, b] = other_axes(axis);
    let shear = unit(a) * amounts[0] + unit(b) * amounts[1];
    let n = unit(axis);
    map_volume(volume.size(), |p| {
        let depth = (p - 0.5).dot(n);
        voxel(volume, p - depth * shear)
    })
}

/// *Rotate*: any angle about `axis`, counter-clockwise looking down the axis
/// from its positive end. The box grows to hold the turned one; cells
/// sample their nearest source cell, `voxel(rot * (dst_p - dst_centre) +
/// src_centre)`, where `rot` is the inverse turn.
pub fn rotate_by(volume: &Volume, axis: usize, degrees: f32) -> Volume {
    let turn = Mat3::from_axis_angle(unit(axis), degrees.to_radians());
    let src = size_vec(volume);
    // The turned box's extent: the sum of the turned half-edges' sizes.
    let half = src * 0.5;
    let extent = turn.x_axis.abs() * half.x + turn.y_axis.abs() * half.y + turn.z_axis.abs() * half.z;
    // A hair under each whole cell, so 90 degrees doesn't round up a row.
    let size = clamp_size((extent * 2.0 - 1e-3).ceil());
    let dst_centre = Vec3::from_array(size.map(|s| s as f32)) * 0.5;
    let inverse = turn.transpose();
    map_volume(size, |p| voxel(volume, inverse * (p - dst_centre) + half))
}

/// *Repeat*: the contents tiled `counts` times on each axis (the box grows,
/// capped at 256): `voxel(mod(dst_p, size))`.
pub fn repeat(volume: &Volume, counts: [u32; 3]) -> Volume {
    let src = size_vec(volume);
    let size = clamp_size(src * Vec3::from_array(counts.map(|c| c.max(1) as f32)));
    map_volume(size, |p| voxel(volume, p % src))
}

/// *Mirror*: the lower half along `axis` copied onto the upper half,
/// reflected, making the model symmetric:
/// `src = abs(size - p); if all(src >= p) src = p`.
pub fn symmetrize(volume: &Volume, axis: usize) -> Volume {
    let size = unit(axis) * size_vec(volume);
    map_volume(volume.size(), |p| {
        let src = (size - p).abs();
        let src = if src.cmpge(p).all() { p } else { src };
        voxel(volume, src)
    })
}

/// *Diagonal*: across `axis`, the half where the first other coordinate is
/// at least the second is copied onto the other half, reflected through the
/// diagonal: `x = dot(axis.zxy, p); y = dot(axis.yzx, p); if x < y, swap`.
/// A box that isn't square across the axis loses what falls outside.
pub fn diagonal(volume: &Volume, axis: usize) -> Volume {
    let n = unit(axis);
    let (s, t) = (Vec3::new(n.z, n.x, n.y), Vec3::new(n.y, n.z, n.x));
    map_volume(volume.size(), |p| {
        let (x, y, z) = (s.dot(p), t.dot(p), n.dot(p));
        let src = if x < y { s * y + t * x + n * z } else { p };
        voxel(volume, src)
    })
}

/// Neighbourhoods for dilation and erosion: faces (6), faces and edges
/// (18), or every touching cell (26).
pub fn neighbours(count: u32) -> Option<Vec<Vec3>> {
    if !matches!(count, 6 | 18 | 26) {
        return None;
    }
    let mut out = Vec::new();
    for z in -1i32..=1 {
        for y in -1i32..=1 {
            for x in -1i32..=1 {
                let touching = x.abs() + y.abs() + z.abs();
                let wanted = match count {
                    6 => touching == 1,
                    18 => touching == 1 || touching == 2,
                    _ => touching > 0,
                };
                if wanted {
                    out.push(Vec3::new(x as f32, y as f32, z as f32));
                }
            }
        }
    }
    Some(out)
}

/// *Dilation*: an empty cell with a filled neighbour becomes `color`.
pub fn dilate(volume: &Volume, color: u8, offsets: &[Vec3]) -> Volume {
    map_volume(volume.size(), |p| {
        let c = voxel(volume, p);
        if c == 0 && offsets.iter().any(|o| voxel(volume, p + *o) > 0) { color } else { c }
    })
}

/// *Erosion*: a filled cell with an empty neighbour -- the outside of the
/// box counts as empty -- is cleared.
pub fn erode(volume: &Volume, offsets: &[Vec3]) -> Volume {
    map_volume(volume.size(), |p| {
        let c = voxel(volume, p);
        if c > 0 && offsets.iter().any(|o| voxel(volume, p + *o) == 0) { 0 } else { c }
    })
}

/// The *Shape* buttons' solids, each filling the whole box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prim {
    Ellipsoid,
    Cylinder,
    Pyramid,
    Cone,
}

impl Prim {
    pub const ALL: [(Prim, &'static str); 4] = [
        (Prim::Ellipsoid, "elli"),
        (Prim::Cylinder, "cyli"),
        (Prim::Pyramid, "pyra"),
        (Prim::Cone, "cone"),
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, n)| *n == name).map(|(p, _)| *p)
    }
}

/// *Shape*: `color` inside the solid, empty outside.
pub fn prim(volume: &Volume, prim: Prim, color: u8) -> Volume {
    let size = size_vec(volume);
    map_volume(volume.size(), |p| {
        let inside = match prim {
            Prim::Ellipsoid => {
                let q = p / size * 2.0 - 1.0;
                q.dot(q) <= 1.0
            }
            Prim::Cylinder => {
                let q = (p / size * 2.0 - 1.0).truncate();
                q.dot(q) <= 1.0
            }
            Prim::Cone => {
                let q = p / size * Vec3::new(2.0, 2.0, 1.0);
                let xy = q.truncate() - 1.0;
                let z = 1.0 - q.z;
                xy.dot(xy) <= z * z
            }
            Prim::Pyramid => {
                let q = (p / size * Vec3::new(2.0, 2.0, 1.0) - Vec3::new(1.0, 1.0, 0.0)).abs();
                q.x + q.z <= 1.0 && q.y + q.z <= 1.0
            }
        };
        if inside { color } else { 0 }
    })
}

/// The pattern repeated over the box, as `repeat` tiles: its cell for `p`.
fn tiled(pattern: &Volume, p: Vec3) -> u8 {
    voxel(pattern, p % size_vec(pattern))
}

/// *Mask*: keeps only the voxels where the pattern, tiled over the box from
/// its corner, is solid.
pub fn mask(volume: &Volume, pattern: &Volume) -> Volume {
    map_volume(volume.size(), |p| if tiled(pattern, p) > 0 { voxel(volume, p) } else { 0 })
}

/// *Texture*: repaints the voxels where the tiled pattern is solid with the
/// pattern's colours.
pub fn texture(volume: &Volume, pattern: &Volume) -> Volume {
    map_volume(volume.size(), |p| {
        let (c, t) = (voxel(volume, p), tiled(pattern, p));
        if c > 0 && t > 0 { t } else { c }
    })
}

/// Which empty cells are open to the outside: reachable from beyond the
/// box through empty cells, moving across faces.
fn outside_air(volume: &Volume) -> Vec<bool> {
    let [sx, sy, sz] = volume.size();
    let index = |x: u32, y: u32, z: u32| (x + sx * (y + sy * z)) as usize;
    let mut open = vec![false; (sx * sy * sz) as usize];
    let mut stack = Vec::new();
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let border = x == 0 || y == 0 || z == 0 || x == sx - 1 || y == sy - 1 || z == sz - 1;
                if border && volume.get(x, y, z) == 0 {
                    open[index(x, y, z)] = true;
                    stack.push([x, y, z]);
                }
            }
        }
    }
    while let Some([x, y, z]) = stack.pop() {
        let p = [x as i64, y as i64, z as i64];
        for axis in 0..3 {
            for step in [-1i64, 1] {
                let mut q = p;
                q[axis] += step;
                if q.iter().zip([sx, sy, sz]).any(|(&c, s)| c < 0 || c >= s as i64) {
                    continue;
                }
                let (qx, qy, qz) = (q[0] as u32, q[1] as u32, q[2] as u32);
                let i = index(qx, qy, qz);
                if !open[i] && volume.get(qx, qy, qz) == 0 {
                    open[i] = true;
                    stack.push([qx, qy, qz]);
                }
            }
        }
    }
    open
}

/// *Hull*: removes every voxel that can't be seen from outside -- one with
/// no face against open air or the box's edge.
pub fn hull(volume: &Volume) -> Volume {
    let [sx, sy, sz] = volume.size();
    let open = outside_air(volume);
    let index = |x: i64, y: i64, z: i64| (x + sx as i64 * (y + sy as i64 * z)) as usize;
    map_volume(volume.size(), |p| {
        let c = voxel(volume, p);
        if c == 0 {
            return 0;
        }
        let at = [p.x as i64, p.y as i64, p.z as i64];
        let seen = (0..3).any(|axis| {
            [-1i64, 1].iter().any(|step| {
                let mut q = at;
                q[axis] += step;
                q.iter().zip([sx, sy, sz]).any(|(&c, s)| c < 0 || c >= s as i64) || open[index(q[0], q[1], q[2])]
            })
        });
        if seen { c } else { 0 }
    })
}

/// *Flood*: fills every enclosed empty space -- cells no path of empty
/// cells connects to the outside -- with `color`.
pub fn flood(volume: &Volume, color: u8) -> Volume {
    let [sx, sy, _] = volume.size();
    let open = outside_air(volume);
    map_volume(volume.size(), |p| {
        let c = voxel(volume, p);
        let i = (p.x as u32 + sx * (p.y as u32 + sy * p.z as u32)) as usize;
        if c == 0 && !open[i] { color } else { c }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(cells: &[[u32; 3]], size: [u32; 3]) -> Volume {
        let mut v = Volume::new(size);
        for &[x, y, z] in cells {
            v.set(x, y, z, 7);
        }
        v
    }

    fn cells(v: &Volume) -> Vec<[u32; 3]> {
        let [sx, sy, sz] = v.size();
        let mut out = Vec::new();
        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    if v.get(x, y, z) != 0 {
                        out.push([x, y, z]);
                    }
                }
            }
        }
        out
    }

    #[test]
    fn full_and_reverse() {
        let v = filled(&[[0, 0, 0]], [2, 1, 1]);
        assert_eq!(cells(&full(&v, 3)).len(), 2);
        let r = reverse(&v, 3);
        assert_eq!(r.get(0, 0, 0), 0);
        assert_eq!(r.get(1, 0, 0), 3);
    }

    #[test]
    fn loop_wraps() {
        let v = filled(&[[3, 0, 0]], [4, 1, 1]);
        assert_eq!(cells(&shift_wrap(&v, 0, 1)), vec![[0, 0, 0]]);
        assert_eq!(cells(&shift_wrap(&v, 0, -2)), vec![[1, 0, 0]]);
    }

    #[test]
    fn double_and_halve() {
        let v = filled(&[[1, 0, 0]], [2, 1, 1]);
        let d = double(&v);
        assert_eq!(d.size(), [4, 2, 2]);
        assert_eq!(cells(&d).len(), 8);
        assert!(cells(&d).iter().all(|c| c[0] >= 2));
        let h = halve(&d);
        assert_eq!(h.size(), [2, 1, 1]);
        assert_eq!(cells(&h), vec![[1, 0, 0]]);
        // Odd sizes round up, and a block keeps any filled cell.
        let odd = filled(&[[2, 0, 0]], [3, 1, 1]);
        assert_eq!(cells(&halve(&odd)), vec![[1, 0, 0]]);
    }

    #[test]
    fn scale_by_axis() {
        let v = filled(&[[0, 0, 0], [1, 1, 1]], [2, 2, 2]);
        let s = scale(&v, Vec3::new(1.0, 1.0, 3.0));
        assert_eq!(s.size(), [2, 2, 6]);
        assert_eq!(cells(&s).len(), 6);
    }

    #[test]
    fn shear_slides_layers() {
        // A column up z, sheared by 1 cell per layer in x.
        let v = filled(&[[0, 0, 0], [0, 0, 1], [0, 0, 2]], [4, 1, 3]);
        assert_eq!(cells(&shear(&v, 2, [1.0, 0.0])), vec![[0, 0, 0], [1, 0, 1], [2, 0, 2]]);
    }

    #[test]
    fn rotate_quarter_keeps_the_box_and_45_grows_it() {
        let v = filled(&[[3, 0, 0]], [4, 2, 1]);
        let q = rotate_by(&v, 2, 90.0);
        assert_eq!(q.size(), [2, 4, 1]);
        assert_eq!(cells(&q).len(), 1);
        // Same as the quarter turn the editor already had.
        assert_eq!(cells(&q), cells(&crate::ops::rotate(&v, 2)));
        let d = rotate_by(&filled(&[], [10, 10, 1]), 2, 45.0);
        assert_eq!(d.size(), [15, 15, 1]);
    }

    #[test]
    fn repeat_tiles() {
        let v = filled(&[[0, 0, 0]], [2, 1, 1]);
        let r = repeat(&v, [3, 2, 1]);
        assert_eq!(r.size(), [6, 2, 1]);
        assert_eq!(cells(&r), vec![[0, 0, 0], [2, 0, 0], [4, 0, 0], [0, 1, 0], [2, 1, 0], [4, 1, 0]]);
    }

    #[test]
    fn mirror_copies_the_lower_half_up() {
        let v = filled(&[[0, 0, 0], [4, 0, 0]], [5, 1, 1]);
        // Cell 0 lands on 4; the old 4, in the upper half, is replaced by
        // its reflection (cell 0), which is filled too.
        assert_eq!(cells(&symmetrize(&v, 0)), vec![[0, 0, 0], [4, 0, 0]]);
        let only_upper = filled(&[[4, 0, 0]], [5, 1, 1]);
        assert!(cells(&symmetrize(&only_upper, 0)).is_empty());
        let lower = filled(&[[1, 0, 0]], [6, 1, 1]);
        assert_eq!(cells(&symmetrize(&lower, 0)), vec![[1, 0, 0], [4, 0, 0]]);
    }

    #[test]
    fn diagonal_reflects_through_x_equals_y() {
        let v = filled(&[[3, 1, 0]], [4, 4, 1]);
        assert_eq!(cells(&diagonal(&v, 2)), vec![[3, 1, 0], [1, 3, 0]]);
    }

    #[test]
    fn dilate_and_erode() {
        let v = filled(&[[1, 1, 1]], [3, 3, 3]);
        let six = neighbours(6).unwrap();
        assert_eq!(cells(&dilate(&v, 7, &six)).len(), 7);
        assert_eq!(cells(&dilate(&v, 7, &neighbours(26).unwrap())).len(), 27);
        let cube = full(&Volume::new([3, 3, 3]), 7);
        assert_eq!(cells(&erode(&cube, &six)), vec![[1, 1, 1]]);
        assert!(neighbours(5).is_none());
    }

    #[test]
    fn prims_fill_the_box() {
        let v = Volume::new([8, 8, 8]);
        let n = |p| cells(&prim(&v, p, 1)).len();
        assert!(n(Prim::Cylinder) > n(Prim::Ellipsoid));
        // A third of the box for the pyramid, a twelfth of pi for the cone.
        assert!(n(Prim::Ellipsoid) > n(Prim::Pyramid));
        assert!(n(Prim::Pyramid) > n(Prim::Cone));
        // The cone narrows upward.
        let cone = prim(&v, Prim::Cone, 1);
        let layer = |z| cells(&cone).iter().filter(|c| c[2] == z).count();
        assert!(layer(0) > layer(6));
    }

    #[test]
    fn hull_and_flood() {
        // A hollow 5³ shell with a voxel inside it.
        let mut shell = full(&Volume::new([5, 5, 5]), 7);
        for z in 1..4 {
            for y in 1..4 {
                for x in 1..4 {
                    shell.set(x, y, z, 0);
                }
            }
        }
        shell.set(2, 2, 2, 9);
        let h = hull(&shell);
        assert_eq!(h.get(2, 2, 2), 0);
        assert_eq!(cells(&h).len(), 125 - 27);
        let f = flood(&shell, 3);
        assert_eq!(f.get(1, 1, 1), 3);
        assert_eq!(f.get(2, 2, 2), 9);
        // An open cup is not enclosed.
        let mut cup = shell.clone();
        cup.set(2, 2, 4, 0);
        assert_eq!(flood(&cup, 3).get(1, 1, 1), 0);
    }

    #[test]
    fn mask_and_texture_tile_the_pattern() {
        // A checker of 2x1x1: solid, empty.
        let pattern = filled(&[[0, 0, 0]], [2, 1, 1]);
        let row = full(&Volume::new([4, 1, 1]), 3);
        assert_eq!(cells(&mask(&row, &pattern)), vec![[0, 0, 0], [2, 0, 0]]);
        let t = texture(&row, &pattern);
        assert_eq!((t.get(0, 0, 0), t.get(1, 0, 0), t.get(2, 0, 0)), (7, 3, 7));
    }
}
