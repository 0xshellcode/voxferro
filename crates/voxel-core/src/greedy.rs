//! Greedy meshing: merging coplanar faces of one colour into rectangles.
//!
//! The face-culling mesher emits one quad per exposed voxel face. A flat wall
//! of one colour is a single rectangle, though, and merging such runs is what
//! the editor's OBJ exporter does (its `merge` option). For each face
//! direction and each slice along it, the exposed faces form a 2D grid of
//! palette indices; rectangles of equal value are grown greedily, first
//! along one axis and then the other.
//!
//! Merging leaves T-junctions where a large rectangle meets several small
//! ones. With integer coordinates the edges are exactly collinear, which is
//! enough for a rasteriser not to show cracks in practice.

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
}

/// Merged quads for a volume. `value` gives each voxel's face value: its
/// palette index, possibly flagged; faces merge only when values match.
pub fn greedy_quads(volume: &Volume, value: impl Fn(u32, u32, u32) -> u16) -> Vec<Quad> {
    let size = volume.size();
    let mut quads = Vec::new();

    for face in Face::ALL {
        let axis = face as usize / 2;
        let positive = face as usize % 2 == 1;
        // In-plane axes, cyclic so that u x v points along +axis.
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        let (nu, nv) = (size[u] as usize, size[v] as usize);
        let step = face.offset();

        let mut mask = vec![0u16; nu * nv];
        for slice in 0..size[axis] {
            // The exposed faces of this slice, 0 where there is none.
            for j in 0..nv {
                for i in 0..nu {
                    let mut p = [0u32; 3];
                    p[axis] = slice;
                    p[u] = i as u32;
                    p[v] = j as u32;
                    let solid = volume.get(p[0], p[1], p[2]) != 0;
                    let n = [p[0] as i32 + step[0], p[1] as i32 + step[1], p[2] as i32 + step[2]];
                    let open = volume.get_i(n[0], n[1], n[2]) == 0;
                    mask[j * nu + i] = if solid && open { value(p[0], p[1], p[2]) } else { 0 };
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
                    while i + w < nu && mask[j * nu + i + w] == m {
                        w += 1;
                    }
                    let mut h = 1;
                    'grow: while j + h < nv {
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

                    let plane = slice as f32 + if positive { 1.0 } else { 0.0 };
                    let corner = |a: usize, b: usize| {
                        let mut c = Vec3::ZERO;
                        c[axis] = plane;
                        c[u] = a as f32;
                        c[v] = b as f32;
                        c
                    };
                    let (u0, u1, v0, v1) = (i, i + w, j, j + h);
                    let ccw = [corner(u0, v0), corner(u1, v0), corner(u1, v1), corner(u0, v1)];
                    let corners = if positive { ccw } else { [ccw[0], ccw[3], ccw[2], ccw[1]] };
                    quads.push(Quad { corners, face, value: m });
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
}
