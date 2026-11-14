//! The Edit panel's operations.
//!
//! Most of these are small kernels: a `map(dst_p)` evaluated at the centre
//! of every cell of the result (`dst_p` is `(x + 0.5, y + 0.5, z + 0.5)`),
//! reading the model through `voxel(p)`, which floors `p` and returns 0
//! outside the box.
//!
//! Hull and flood are not kernels and are written from what their buttons
//! promise.

use crate::Volume;
use glam::{IVec3, Mat3, Vec3};

/// Evaluates `map` at every cell centre of a `size` box.
fn map_volume(size: [u32; 3], map: impl Fn(Vec3) -> u8 + Sync) -> Volume {
    let [sx, sy, sz] = size.map(|s| s as usize);
    let mut cells = vec![0u8; sx * sy * sz];
    // Layers are independent: split them among the cores.
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(sz.max(1));
    let layers_each = sz.div_ceil(threads.max(1)).max(1);
    std::thread::scope(|scope| {
        for (k, slab) in cells.chunks_mut((layers_each * sx * sy).max(1)).enumerate() {
            let map = &map;
            scope.spawn(move || {
                for (i, cell) in slab.iter_mut().enumerate() {
                    let (x, y, z) = (i % sx, (i / sx) % sy, k * layers_each + i / (sx * sy));
                    *cell = map(Vec3::new(x as f32, y as f32, z as f32) + 0.5);
                }
            });
        }
    });
    Volume::from_cells(size, cells)
}

/// The Transform *wrap*: `source` repeated to
/// fill a box `size` cells big whose corner is `offset` cells from the
/// source's. The copies start at the source's own place and repeat every
/// `size + spacing` cells along each axis, both ways; the gap between them
/// is empty, and a box smaller than the source crops it. With `mirror` on
/// an axis, every other copy along it is reflected.
pub fn wrap(source: &Volume, offset: IVec3, size: [u32; 3], spacing: [u32; 3], mirror: [bool; 3]) -> Volume {
    let src = source.size().map(|s| s as i32);
    map_volume(size, |c| {
        let p = c.floor().as_ivec3() + offset;
        let mut at = [0u32; 3];
        for a in 0..3 {
            let period = src[a] + spacing[a] as i32;
            let r = p[a].rem_euclid(period);
            if r >= src[a] {
                return 0;
            }
            let odd = p[a].div_euclid(period).rem_euclid(2) == 1;
            at[a] = if mirror[a] && odd { src[a] - 1 - r } else { r } as u32;
        }
        source.get(at[0], at[1], at[2])
    })
}

/// A small, fast random source for the kernels that need one (Maze), so a
/// seed gives the same model every time.
pub struct Rand(u64);

impl Rand {
    pub fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    /// A seed from the clock, for a new model on each press.
    pub fn from_clock() -> Self {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        Self::new(t)
    }

    pub fn next_u32(&mut self) -> u32 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 32) as u32
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u32() as usize) % n.max(1)
    }
}

/// The *Maze*: a three-dimensional maze of
/// one-voxel corridors in `colour`, filling a box the size of `size`; what
/// was there is replaced. Cells sit every `step = max(p, 1) + 2` voxels
/// (3 by default) and corridors join them, grown like this:
/// a list of open cells, half the time the newest taken and half the time
/// one at random; from it a step into an unvisited neighbour, toward -x,
/// -y or -z first and toward + only when those are used up or one time in
/// five; a cell with nowhere to go leaves the list.
pub fn maze(size: [u32; 3], colour: u8, p: u32, rand: &mut Rand) -> Volume {
    let step = p.max(1) as i32 + 2;
    let mut out = Volume::new(size);
    let n = size.map(|s| (s as i32 - 1).max(0) / step + 1);
    let index = |c: IVec3| (c.x + n[0] * (c.y + n[1] * c.z)) as usize;
    let inside = |c: IVec3| (0..3).all(|a| c[a] >= 0 && c[a] < n[a]);
    let mut visited = vec![false; (n[0] * n[1] * n[2]) as usize];
    let put = |out: &mut Volume, p: IVec3| {
        if p.cmpge(IVec3::ZERO).all() {
            out.set(p.x as u32, p.y as u32, p.z as u32, colour);
        }
    };
    let start = IVec3::new(rand.below(n[0] as usize) as i32, rand.below(n[1] as usize) as i32, rand.below(n[2] as usize) as i32);
    visited[index(start)] = true;
    put(&mut out, start * step);
    let mut open = vec![start];
    let back = [IVec3::NEG_X, IVec3::NEG_Y, IVec3::NEG_Z];
    let ahead = [IVec3::X, IVec3::Y, IVec3::Z];
    while !open.is_empty() {
        let newest = rand.next_u32() & 1 == 1;
        let i = if newest { open.len() - 1 } else { rand.below(open.len()) };
        let c = open[i];
        let free = |d: &IVec3| inside(c + *d) && !visited[index(c + *d)];
        let mut ways: Vec<IVec3> = back.iter().copied().filter(free).collect();
        if ways.is_empty() || rand.below(5) == 0 {
            ways.extend(ahead.iter().copied().filter(free));
        }
        if ways.is_empty() {
            open.swap_remove(i);
            continue;
        }
        let d = ways[rand.below(ways.len())];
        visited[index(c + d)] = true;
        open.push(c + d);
        for t in 1..=step {
            put(&mut out, c * step + d * t);
        }
    }
    out
}

/// Ken Perlin's improved noise: the
/// reference permutation, the quintic fade and the twelve edge gradients.
pub fn perlin(p: Vec3) -> f32 {
    const PERM: [u8; 256] = [
        151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69, 142, 8, 99, 37, 240, 21, 10, 23, 190,
        6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219, 203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 20, 125,
        136, 171, 168, 68, 175, 74, 165, 71, 134, 139, 48, 27, 166, 77, 146, 158, 231, 83, 111, 229, 122, 60, 211, 133, 230, 220, 105, 92,
        41, 55, 46, 245, 40, 244, 102, 143, 54, 65, 25, 63, 161, 1, 216, 80, 73, 209, 76, 132, 187, 208, 89, 18, 169, 200, 196, 135, 130,
        116, 188, 159, 86, 164, 100, 109, 198, 173, 186, 3, 64, 52, 217, 226, 250, 124, 123, 5, 202, 38, 147, 118, 126, 255, 82, 85, 212,
        207, 206, 59, 227, 47, 16, 58, 17, 182, 189, 28, 42, 223, 183, 170, 213, 119, 248, 152, 2, 44, 154, 163, 70, 221, 153, 101, 155,
        167, 43, 172, 9, 129, 22, 39, 253, 19, 98, 108, 110, 79, 113, 224, 232, 178, 185, 112, 104, 218, 246, 97, 228, 251, 34, 242, 193,
        238, 210, 144, 12, 191, 179, 162, 241, 81, 51, 145, 235, 249, 14, 239, 107, 49, 192, 214, 31, 181, 199, 106, 157, 184, 84, 204,
        176, 115, 121, 50, 45, 127, 4, 150, 254, 138, 236, 205, 93, 222, 114, 67, 29, 24, 72, 243, 141, 128, 195, 78, 66, 215, 61, 156, 180,
    ];
    const GRAD: [[f32; 3]; 16] = [
        [0.0, 1.0, 1.0], [0.0, 1.0, -1.0], [0.0, -1.0, -1.0], [0.0, -1.0, 1.0], [1.0, 0.0, 1.0], [1.0, 0.0, -1.0], [-1.0, 0.0, -1.0],
        [-1.0, 0.0, 1.0], [1.0, 1.0, 0.0], [1.0, -1.0, 0.0], [-1.0, -1.0, 0.0], [-1.0, 1.0, 0.0], [0.0, 1.0, 1.0], [0.0, 1.0, -1.0],
        [0.0, -1.0, -1.0], [0.0, -1.0, 1.0],
    ];
    let perm = |i: i32| PERM[(i & 255) as usize] as i32;
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let lerp = |t: f32, a: f32, b: f32| a + t * (b - a);
    let cell = p.floor();
    let [x, y, z] = cell.as_ivec3().to_array();
    let f = p - cell;
    let grad = |h: i32, d: Vec3| Vec3::from_array(GRAD[(h & 15) as usize]).dot(d);
    let (a, b) = (perm(x) + y, perm(x + 1) + y);
    let (aa, ab, ba, bb) = (perm(a) + z, perm(a + 1) + z, perm(b) + z, perm(b + 1) + z);
    let (u, v, w) = (fade(f.x), fade(f.y), fade(f.z));
    let c = |h: i32, dx: f32, dy: f32, dz: f32| grad(perm(h), f - Vec3::new(dx, dy, dz));
    lerp(
        w,
        lerp(v, lerp(u, c(aa, 0.0, 0.0, 0.0), c(ba, 1.0, 0.0, 0.0)), lerp(u, c(ab, 0.0, 1.0, 0.0), c(bb, 1.0, 1.0, 0.0))),
        lerp(v, lerp(u, c(aa + 1, 0.0, 0.0, 1.0), c(ba + 1, 1.0, 0.0, 1.0)), lerp(u, c(ab + 1, 0.0, 1.0, 1.0), c(bb + 1, 1.0, 1.0, 1.0))),
    )
}

/// The *Perlin*: the box filled in `colour`
/// where `sin(noise · π)` falls strictly between `lo` and `hi`, the noise
/// sampled at `(cell + seed) · scale` -- a band of the noise, which reads as
/// winding caves and sheets. What was there is replaced. The defaults:
/// seed 0, scale 0.04, band 0.2 to 0.5.
pub fn noise(size: [u32; 3], colour: u8, seed: u32, scale: f32, lo: f32, hi: f32) -> Volume {
    let seed = seed as f32;
    map_volume(size, |c| {
        let v = (perlin((c.floor() + seed) * scale) * std::f32::consts::PI).sin();
        if lo < v && v < hi { colour } else { 0 }
    })
}

/// The *Fractal*: each voxel becomes the
/// pattern's shape in the voxel's colour, so the model grows by the
/// pattern's size on each axis (up to 256). The pattern's own colours are
/// not used.
pub fn fractal(volume: &Volume, pattern: &Volume) -> Volume {
    let p = pattern.size();
    let src = volume.size();
    let size = [0, 1, 2].map(|a| (src[a] * p[a]).clamp(1, 256));
    map_volume(size, |c| {
        let q = c.floor().as_uvec3();
        let s = volume.get(q.x / p[0], q.y / p[1], q.z / p[2]);
        if s != 0 && pattern.get(q.x % p[0], q.y % p[1], q.z % p[2]) != 0 { s } else { 0 }
    })
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
    fn a_maze_joins_every_cell_by_corridors() {
        let size = [13, 13, 7];
        let m = maze(size, 9, 1, &mut Rand::new(7));
        // Cells every 3 voxels: 5 x 5 x 3, all reached.
        for z in (0..7).step_by(3) {
            for y in (0..13).step_by(3) {
                for x in (0..13).step_by(3) {
                    assert_eq!(m.get(x, y, z), 9, "cell {x} {y} {z}");
                }
            }
        }
        // A spanning tree: 75 cells, 74 corridors of 2 voxels between them.
        assert_eq!(m.filled_count(), 75 + 74 * 2);
        // Off the lattice lines nothing is filled.
        assert_eq!(m.get(1, 1, 0), 0);
        // The same seed, the same maze.
        assert_eq!(maze(size, 9, 1, &mut Rand::new(7)), m);
    }

    #[test]
    fn perlin_is_zero_on_the_lattice_and_smooth_between() {
        assert_eq!(perlin(Vec3::new(3.0, 5.0, 7.0)), 0.0);
        let a = perlin(Vec3::new(0.3, 0.4, 0.5));
        let b = perlin(Vec3::new(0.301, 0.4, 0.5));
        assert!((a - b).abs() < 0.01 && a.abs() <= 1.0);
        let n = noise([32, 32, 32], 4, 0, 0.04, 0.2, 0.5);
        let filled = n.filled_count();
        assert!(filled > 0 && filled < 32 * 32 * 32, "{filled}");
        assert_ne!(noise([32, 32, 32], 4, 99, 0.04, 0.2, 0.5), n, "another seed, another model");
    }

    #[test]
    fn fractal_puts_the_pattern_in_each_voxel() {
        let mut v = Volume::new([2, 1, 1]);
        v.set(0, 0, 0, 5);
        let mut cross = Volume::new([3, 3, 1]);
        for (x, y) in [(1, 0), (0, 1), (1, 1), (2, 1), (1, 2)] {
            cross.set(x, y, 0, 77);
        }
        let f = fractal(&v, &cross);
        assert_eq!(f.size(), [6, 3, 1]);
        assert_eq!(f.filled_count(), 5);
        assert_eq!(f.get(1, 1, 0), 5, "the voxel's colour, not the pattern's");
        assert_eq!(f.get(4, 1, 0), 0, "an empty voxel stays empty");
        // Capped at 256 a side.
        assert_eq!(fractal(&Volume::new([100, 1, 1]), &cross).size()[0], 256);
    }

    #[test]
    fn wrap_tiles_with_gaps_mirrors_and_crops() {
        // A 2-long bar along x: colours 1, 2.
        let mut v = Volume::new([2, 1, 1]);
        v.set(0, 0, 0, 1);
        v.set(1, 0, 0, 2);
        let row = |w: &Volume| (0..w.size()[0]).map(|x| w.get(x, 0, 0)).collect::<Vec<_>>();
        let plain = wrap(&v, IVec3::ZERO, [7, 1, 1], [1, 0, 0], [false; 3]);
        assert_eq!(row(&plain), vec![1, 2, 0, 1, 2, 0, 1]);
        let mirrored = wrap(&v, IVec3::ZERO, [7, 1, 1], [1, 0, 0], [true, false, false]);
        assert_eq!(row(&mirrored), vec![1, 2, 0, 2, 1, 0, 1]);
        // Grown the other way: the copies go on before the source.
        let back = wrap(&v, IVec3::new(-3, 0, 0), [5, 1, 1], [1, 0, 0], [false; 3]);
        assert_eq!(row(&back), vec![1, 2, 0, 1, 2]);
        // Smaller than the source: cropped.
        assert_eq!(row(&wrap(&v, IVec3::ZERO, [1, 1, 1], [0; 3], [false; 3])), vec![1]);
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
