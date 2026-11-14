//! Greedy meshing: merging coplanar faces of one colour into rectangles.
//!
//! The face-culling mesher emits one quad per exposed voxel face. A flat wall
//! of one colour is a single rectangle, though, and merging such runs is what
//! the OBJ export does. For each face
//! direction and each slice along it, the exposed faces form a 2D grid of
//! palette indices; rectangles of equal value are grown greedily, first
//! along one axis and then the other.
//!
//! Merging leaves T-junctions where a large rectangle meets several small
//! ones. With integer coordinates the edges are exactly collinear, which is
//! enough for a rasteriser not to show cracks in practice.
//!
//! With ambient occlusion, each face corner also counts the solid cells
//! around it in front of the face:
//! the two beside the corner and the one diagonal to it, 0 to 3. Faces
//! merge only when all four corners agree and match their neighbours', so
//! a merged rectangle is evenly lit and a shaded face keeps its gradient.

use crate::{Face, Volume};
use glam::Vec3;

/// One merged rectangle, in the model's own voxel coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    /// Counter-clockwise seen from outside, like the culling mesher's.
    pub corners: [Vec3; 4],
    pub face: Face,
    /// Palette index, plus 256 when the voxels were marked (see
    /// `Mesh::append_volume_marked`).
    pub value: u16,
    /// Occlusion at each corner, in the order of `corners`: how many of the
    /// three cells around it in front of the face are solid. Zero without
    /// ambient occlusion.
    pub occlusion: [u8; 4],
}

/// Merged quads for a volume. `value` gives each voxel's face value: its
/// palette index, possibly flagged; faces merge only when values match.
pub fn greedy_quads(volume: &Volume, value: impl Fn(u32, u32, u32) -> u16) -> Vec<Quad> {
    greedy_quads_in(volume, [0; 3], volume.size(), value)
}

/// [`greedy_quads`] over the box `lo..hi` of the volume only: one chunk.
/// Faces are still culled against voxels outside the box, so the chunks of
/// a volume together give the same surface as the whole; rectangles just do
/// not merge across chunk borders.
pub fn greedy_quads_in(volume: &Volume, lo: [u32; 3], hi: [u32; 3], value: impl Fn(u32, u32, u32) -> u16) -> Vec<Quad> {
    greedy_quads_shaded(volume, lo, hi, value, false)
}

/// [`greedy_quads_in`], with each corner's ambient occlusion when `occlude`.
pub fn greedy_quads_shaded(
    volume: &Volume,
    lo: [u32; 3],
    hi: [u32; 3],
    value: impl Fn(u32, u32, u32) -> u16,
    occlude: bool,
) -> Vec<Quad> {
    let size = [hi[0].saturating_sub(lo[0]), hi[1].saturating_sub(lo[1]), hi[2].saturating_sub(lo[2])];
    let mut quads = Vec::new();

    for face in Face::ALL {
        let axis = face as usize / 2;
        let positive = face as usize % 2 == 1;
        // In-plane axes, cyclic so that u x v points along +axis.
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let (nu, nv) = (size[u] as usize, size[v] as usize);
        let step = face.offset();

        // A face's value, and above it its occlusion: 1 + the count when
        // all four corners agree, or SHADED when they differ -- such a face
        // stays a quad of its own.
        const SHADED: u32 = 1 << 31;
        let solid_at = |c: [i32; 3]| volume.get_i(c[0], c[1], c[2]) != 0;
        // The count at corner (du, dv) of the face of cell `p`.
        let corner_occlusion = |p: [u32; 3], du: i32, dv: i32| -> u8 {
            let mut n = [p[0] as i32 + step[0], p[1] as i32 + step[1], p[2] as i32 + step[2]];
            let (su, sv) = (if du == 1 { 1 } else { -1 }, if dv == 1 { 1 } else { -1 });
            let mut a = n;
            a[u] += su;
            let mut b = n;
            b[v] += sv;
            n[u] += su;
            n[v] += sv;
            solid_at(a) as u8 + solid_at(b) as u8 + solid_at(n) as u8
        };
        let corners_of = |p: [u32; 3]| [(0, 0), (1, 0), (1, 1), (0, 1)].map(|(du, dv)| corner_occlusion(p, du, dv));
        let mut mask = vec![0u32; nu * nv];
        for slice in 0..size[axis] {
            // The exposed faces of this slice, 0 where there is none.
            for j in 0..nv {
                for i in 0..nu {
                    let mut p = [0u32; 3];
                    p[axis] = slice + lo[axis];
                    p[u] = i as u32 + lo[u];
                    p[v] = j as u32 + lo[v];
                    let solid = volume.get(p[0], p[1], p[2]) != 0;
                    let n = [p[0] as i32 + step[0], p[1] as i32 + step[1], p[2] as i32 + step[2]];
                    let open = volume.get_i(n[0], n[1], n[2]) == 0;
                    mask[j * nu + i] = if !(solid && open) {
                        0
                    } else if !occlude {
                        value(p[0], p[1], p[2]) as u32
                    } else {
                        let c = corners_of(p);
                        let shade = if c.iter().all(|&x| x == c[0]) { (1 + c[0] as u32) << 16 } else { SHADED };
                        value(p[0], p[1], p[2]) as u32 | shade
                    };
                }
            }

            // Grow rectangles: as wide as the run allows, then as tall as
            // every row of that width matches.
            for j in 0..nv {
                let mut i = 0;
                while i < nu {
                    let m = mask[j * nu + i];
                    if m == 0 {
                        i += 1;
                        continue;
                    }
                    let mut w = 1;
                    while m & SHADED == 0 && i + w < nu && mask[j * nu + i + w] == m {
                        w += 1;
                    }
                    let mut h = 1;
                    'grow: while m & SHADED == 0 && j + h < nv {
                        for k in 0..w {
                            if mask[(j + h) * nu + i + k] != m {
                                break 'grow;
                            }
                        }
                        h += 1;
                    }
                    for row in 0..h {
                        for k in 0..w {
                            mask[(j + row) * nu + i + k] = 0;
                        }
                    }

                    let plane = (slice + lo[axis]) as f32 + if positive { 1.0 } else { 0.0 };
                    let corner = |a: usize, b: usize| {
                        let mut c = Vec3::ZERO;
                        c[axis] = plane;
                        c[u] = (a as u32 + lo[u]) as f32;
                        c[v] = (b as u32 + lo[v]) as f32;
                        c
                    };
                    let (u0, u1, v0, v1) = (i, i + w, j, j + h);
                    let ccw = [corner(u0, v0), corner(u1, v0), corner(u1, v1), corner(u0, v1)];
                    let corners = if positive { ccw } else { [ccw[0], ccw[3], ccw[2], ccw[1]] };
                    let occlusion = if m & SHADED != 0 {
                        let mut p = [0u32; 3];
                        p[axis] = slice + lo[axis];
                        p[u] = i as u32 + lo[u];
                        p[v] = j as u32 + lo[v];
                        corners_of(p)
                    } else {
                        [((m >> 16) & 0xff).saturating_sub(1) as u8; 4]
                    };
                    let occlusion = if positive { occlusion } else { [occlusion[0], occlusion[3], occlusion[2], occlusion[1]] };
                    quads.push(Quad { corners, face, value: m as u16, occlusion });
                    i += w;
                }
            }
        }
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mesh;

    fn index_of(v: &Volume) -> impl Fn(u32, u32, u32) -> u16 + '_ {
        move |x, y, z| v.get(x, y, z) as u16
    }

    fn area(q: &Quad) -> f32 {
        let a = q.corners[1] - q.corners[0];
        let b = q.corners[3] - q.corners[0];
        a.cross(b).length()
    }

    #[test]
    fn a_solid_block_is_six_quads() {
        let mut v = Volume::new([5, 4, 3]);
        for z in 0..3 {
            for y in 0..4 {
                for x in 0..5 {
                    v.set(x, y, z, 7);
                }
            }
        }
        let quads = greedy_quads(&v, index_of(&v));
        assert_eq!(quads.len(), 6);
        let total: f32 = quads.iter().map(area).sum();
        assert_eq!(total, 2.0 * (5.0 * 4.0 + 5.0 * 3.0 + 4.0 * 3.0));
    }

    #[test]
    fn colours_do_not_merge() {
        let mut v = Volume::new([2, 1, 1]);
        v.set(0, 0, 0, 1);
        v.set(1, 0, 0, 2);
        let quads = greedy_quads(&v, index_of(&v));
        // Top, bottom, front and back split by colour; the two ends whole.
        assert_eq!(quads.len(), 4 * 2 + 2);
    }

    #[test]
    fn covers_the_same_surface_as_the_culling_mesher() {
        // An irregular shape: the merged area must equal the face count.
        let mut v = Volume::new([6, 6, 6]);
        for (x, y, z) in [(0, 0, 0), (1, 0, 0), (2, 0, 0), (1, 1, 0), (1, 1, 1), (4, 4, 4), (3, 3, 0)] {
            v.set(x, y, z, 3);
        }
        let quads = greedy_quads(&v, index_of(&v));
        let merged: f32 = quads.iter().map(area).sum();
        let faces = Mesh::from_volume(&v).triangle_count() / 2;
        assert_eq!(merged, faces as f32);
    }

    #[test]
    fn winding_faces_outward() {
        let mut v = Volume::new([3, 3, 3]);
        v.set(1, 1, 1, 1);
        for q in greedy_quads(&v, index_of(&v)) {
            let n = (q.corners[1] - q.corners[0]).cross(q.corners[2] - q.corners[0]);
            let o = q.face.offset();
            let expected = Vec3::new(o[0] as f32, o[1] as f32, o[2] as f32);
            assert!(n.normalize().dot(expected) > 0.99, "{:?} winds inward", q.face);
        }
    }

    #[test]
    fn marked_voxels_stay_separate() {
        let mut v = Volume::new([2, 1, 1]);
        v.set(0, 0, 0, 1);
        v.set(1, 0, 0, 1);
        let quads = greedy_quads(&v, |x, _, _| if x == 0 { 1 + 256 } else { 1 });
        assert_eq!(quads.len(), 4 * 2 + 2);
    }

    #[test]
    fn chunked_meshing_covers_the_same_surface() {
        // A shape crossing chunk borders: the chunks together must give the
        // same area as meshing the whole at once.
        let mut v = Volume::new([70, 40, 33]);
        for z in 0..33 {
            for y in 5..37 {
                for x in 10..66 {
                    if (x + y + z) % 7 != 0 {
                        v.set(x, y, z, 1 + (x / 20) as u8);
                    }
                }
            }
        }
        let whole: f32 = greedy_quads(&v, index_of(&v)).iter().map(area).sum();
        let [cx, cy, cz] = v.chunk_dims();
        let size = v.size();
        let mut chunked = 0.0;
        for k in 0..cz {
            for j in 0..cy {
                for i in 0..cx {
                    let lo = [i * crate::volume::CHUNK, j * crate::volume::CHUNK, k * crate::volume::CHUNK];
                    let hi = [0, 1, 2].map(|a| (lo[a] + crate::volume::CHUNK).min(size[a]));
                    chunked += greedy_quads_in(&v, lo, hi, index_of(&v)).iter().map(area).sum::<f32>();
                }
            }
        }
        assert_eq!(chunked, whole);
    }

    #[test]
    fn occlusion_counts_the_cells_around_each_corner() {
        // A floor of 3x3 with one voxel on its middle: the floor's top faces
        // next to it are shaded on the corners that touch it.
        let mut v = Volume::new([3, 3, 2]);
        for y in 0..3 {
            for x in 0..3 {
                v.set(x, y, 0, 1);
            }
        }
        v.set(1, 1, 1, 1);
        let quads = greedy_quads_shaded(&v, [0; 3], v.size(), |x, y, z| v.get(x, y, z) as u16, true);
        let up: Vec<&Quad> = quads.iter().filter(|q| q.face == Face::PosZ && q.corners[0].z == 1.0).collect();
        // The face beside the block on +x: its two corners against it see
        // one side cell each.
        let spans = |q: &Quad, x: f32, y: f32| {
            let lo = q.corners.iter().fold(Vec3::splat(f32::MAX), |a, &c| a.min(c));
            let hi = q.corners.iter().fold(Vec3::splat(f32::MIN), |a, &c| a.max(c));
            lo.x == x && lo.y == y && hi.x == x + 1.0 && hi.y == y + 1.0
        };
        let beside = up.iter().find(|q| spans(q, 2.0, 1.0)).unwrap();
        let against: Vec<u8> = beside.corners.iter().zip(beside.occlusion).filter(|(c, _)| c.x == 2.0 && (c.y == 1.0 || c.y == 2.0)).map(|(_, o)| o).collect();
        assert_eq!(against, [1, 1]);
        // A diagonal neighbour alone counts too.
        let diagonal = up.iter().find(|q| spans(q, 2.0, 2.0)).unwrap();
        assert_eq!(diagonal.occlusion.iter().filter(|&&o| o == 1).count(), 1);
        // Without occlusion, everything merges as before.
        assert!(greedy_quads(&v, |x, y, z| v.get(x, y, z) as u16).iter().all(|q| q.occlusion == [0; 4]));
        // Every face of the open sides of the block's top is unshaded and
        // merged; shaded faces are unit squares.
        for q in &quads {
            let side = (q.corners[2] - q.corners[0]).abs();
            if q.occlusion.iter().any(|&o| o != q.occlusion[0]) {
                assert_eq!(side.x + side.y + side.z, 2.0, "a shaded face is one voxel");
            }
        }
    }
}
