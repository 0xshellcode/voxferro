//! The *mc* export: a smooth surface over the voxels, by marching cubes
//! without interpolation.
//!
//! Each cell of eight voxel centres becomes an 8-bit case; a table gives the
//! case polygons (3 to 6 sides) whose corners are the midpoints of the cell's
//! crossed edges; each corner takes the colour of the solid voxel of its
//! edge; polygons whose corners differ in colour are split so each colour
//! keeps its own part; triangles go to an ASCII PLY.
//!
//! The table is built: on each face of the cell the crossed edges are joined
//! so that solid corners are separated from empty ones, and the face
//! segments are linked into loops, wound counter-clockwise seen from the
//! empty side. Mixed colours are split the simplest way that keeps colours
//! apart and the surface closed (see `split`).

use crate::{Instance, Scene};
use glam::Vec3;
use std::fmt::Write;
use std::sync::OnceLock;
use vox_format::Palette;

/// Corner `i` of a cell is `(i & 1, i >> 1 & 1, i >> 2 & 1)`; the edges
/// are pairs of corners, numbered in this order by the case table.
pub const EDGES: [[usize; 2]; 12] =
    [[0, 1], [2, 3], [4, 5], [6, 7], [0, 2], [4, 6], [1, 3], [5, 7], [0, 4], [1, 5], [2, 6], [3, 7]];

/// The cell's faces as corner loops, counter-clockwise seen from outside.
const FACES: [[usize; 4]; 6] = [[0, 4, 6, 2], [1, 3, 7, 5], [0, 1, 5, 4], [2, 6, 7, 3], [0, 2, 3, 1], [4, 5, 7, 6]];

fn corner(i: usize) -> Vec3 {
    Vec3::new((i & 1) as f32, (i >> 1 & 1) as f32, (i >> 2 & 1) as f32)
}

fn edge_of(a: usize, b: usize) -> usize {
    let key = [a.min(b), a.max(b)];
    EDGES.iter().position(|e| *e == key).expect("an edge of the cube")
}

/// The midpoint of edge `e`, in cell units.
pub fn edge_mid(e: usize) -> Vec3 {
    (corner(EDGES[e][0]) + corner(EDGES[e][1])) * 0.5
}

/// The polygons of one case, as loops of edge numbers.
fn build_case(case: u8) -> Vec<Vec<usize>> {
    let solid = |i: usize| case >> i & 1 == 1;
    // Directed segments across each face: from where the face's loop
    // enters the solid to where it leaves, so the linked loops run
    // counter-clockwise seen from the empty side.
    let mut next = [usize::MAX; 12];
    for face in FACES {
        let crossings: Vec<(usize, bool)> = (0..4)
            .filter_map(|k| {
                let (a, b) = (face[k], face[(k + 1) % 4]);
                (solid(a) != solid(b)).then(|| (edge_of(a, b), solid(a)))
            })
            .collect();
        // Four crossings: two diagonal solid corners. Each solid corner is
        // cut off on its own (the segments pair around the solid ones).
        let pairs: Vec<(usize, usize)> = match crossings.len() {
            2 => vec![(0, 1)],
            4 if crossings[0].1 => vec![(3, 0), (1, 2)],
            4 => vec![(0, 1), (2, 3)],
            _ => Vec::new(),
        };
        for (p, q) in pairs {
            let ((e0, leaves0), (e1, _)) = (crossings[p], crossings[q]);
            let (from, to) = if leaves0 { (e1, e0) } else { (e0, e1) };
            next[from] = to;
        }
    }
    let mut loops = Vec::new();
    let mut seen = [false; 12];
    for start in 0..12 {
        if next[start] == usize::MAX || seen[start] {
            continue;
        }
        let mut lp = vec![start];
        seen[start] = true;
        let mut e = next[start];
        while e != start && e != usize::MAX && !seen[e] {
            lp.push(e);
            seen[e] = true;
            e = next[e];
        }
        loops.push(lp);
    }
    loops
}

/// Every case's polygons, built once.
pub fn table() -> &'static [Vec<Vec<usize>>; 256] {
    static TABLE: OnceLock<[Vec<Vec<usize>>; 256]> = OnceLock::new();
    TABLE.get_or_init(|| std::array::from_fn(|c| build_case(c as u8)))
}

/// One coloured triangle.
type Tri = ([Vec3; 3], u8);

/// A polygon's triangles, each of one colour. A side whose two corners
/// differ in colour is cut at its midpoint, each half going with its own
/// corner -- a rule both polygons sharing the side apply alike, since it
/// depends on the side alone -- and the pieces are fanned from the centre.
/// A plain triangle is kept whole.
fn split(points: &[Vec3], colours: &[u8], out: &mut Vec<Tri>) {
    let n = points.len();
    if n == 3 && colours.iter().all(|&c| c == colours[0]) {
        out.push(([points[0], points[1], points[2]], colours[0]));
        return;
    }
    let centre = points.iter().copied().sum::<Vec3>() / n as f32;
    for i in 0..n {
        let j = (i + 1) % n;
        if colours[i] == colours[j] {
            out.push(([points[i], points[j], centre], colours[i]));
        } else {
            let mid = (points[i] + points[j]) * 0.5;
            out.push(([points[i], mid, centre], colours[i]));
            out.push(([mid, points[j], centre], colours[j]));
        }
    }
}

/// The surface's triangles for the instances `keep` accepts, in the world.
pub fn triangles(scene: &Scene, keep: impl Fn(&Instance) -> bool) -> Vec<Tri> {
    let table = table();
    let mut out = Vec::new();
    for instance in scene.instances.iter().filter(|i| keep(i)) {
        let Some(volume) = scene.models.get(instance.model_index) else { continue };
        let pivot = volume.pivot();
        let [sx, sy, sz] = volume.size().map(|s| s as i32);
        let mut points = Vec::with_capacity(6);
        let mut colours = Vec::with_capacity(6);
        let mut local = Vec::new();
        // Cells span voxel centres; one beyond each side closes the surface.
        for z in -1..sz {
            for y in -1..sy {
                for x in -1..sx {
                    let value = |i: usize| volume.get_i(x + (i & 1) as i32, y + (i >> 1 & 1) as i32, z + (i >> 2 & 1) as i32);
                    let values: [u8; 8] = std::array::from_fn(value);
                    let case = values.iter().enumerate().fold(0u8, |m, (i, &v)| m | (((v != 0) as u8) << i));
                    if case == 0 || case == 255 {
                        continue;
                    }
                    // Voxel centres are at index + 0.5.
                    let origin = Vec3::new(x as f32, y as f32, z as f32) + 0.5;
                    for poly in &table[case as usize] {
                        points.clear();
                        colours.clear();
                        for &e in poly {
                            let [a, b] = EDGES[e];
                            points.push(origin + edge_mid(e));
                            colours.push(values[a].max(values[b]));
                        }
                        local.clear();
                        split(&points, &colours, &mut local);
                        let mirrored = instance.transform.rotation.determinant() < 0.0;
                        for (mut tri, c) in local.drain(..) {
                            if mirrored {
                                tri.swap(1, 2);
                            }
                            out.push((tri.map(|p| instance.transform.apply(p - pivot)), c));
                        }
                    }
                }
            }
        }
    }
    out
}

/// The ASCII PLY of the *mc* export: a coloured vertex per triangle corner,
/// `3 a b c` per triangle; positions are multiplied by `scale`, Z up.
pub fn ply(scene: &Scene, palette: &Palette, keep: impl Fn(&Instance) -> bool, scale: f32) -> String {
    let tris = triangles(scene, keep);
    let mut out = String::from("ply\nformat ascii 1.0\ncomment : Voxferro\n");
    let _ = writeln!(out, "element vertex {}", tris.len() * 3);
    out.push_str("property float x\nproperty float y\nproperty float z\n");
    out.push_str("property uchar red\nproperty uchar green\nproperty uchar blue\n");
    let _ = writeln!(out, "element face {}\nproperty list uchar int vertex_index\nend_header", tris.len());
    for (tri, c) in &tris {
        let c = palette.color(*c);
        for p in tri {
            let p = *p * scale;
            let _ = writeln!(out, "{} {} {} {} {} {}", p.x, p.y, p.z, c.r, c.g, c.b);
        }
    }
    for k in 0..tris.len() {
        let _ = writeln!(out, "3 {} {} {}", 3 * k, 3 * k + 1, 3 * k + 2);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Volume;
    use std::collections::HashMap;

    fn scene_of(v: Volume) -> Scene {
        let mut scene = Scene::default();
        scene.models.push(v);
        scene.instances.push(Instance { model_index: 0, ..Default::default() });
        scene
    }

    /// Every edge of the mesh is shared by exactly two triangles, in
    /// opposite directions: closed, and consistently wound.
    fn closed(tris: &[Tri]) -> bool {
        let key = |p: Vec3| (p * 4.0).round().as_ivec3().to_array();
        let mut edges: HashMap<([i32; 3], [i32; 3]), i32> = HashMap::new();
        for (t, _) in tris {
            for i in 0..3 {
                let (a, b) = (key(t[i]), key(t[(i + 1) % 3]));
                *edges.entry((a, b)).or_default() += 1;
            }
        }
        edges.iter().all(|((a, b), n)| *n == 1 && edges.get(&(*b, *a)) == Some(&1))
    }

    #[test]
    fn a_voxel_becomes_a_closed_octahedron_facing_out() {
        let mut v = Volume::new([1, 1, 1]);
        v.set(0, 0, 0, 7);
        let tris = triangles(&scene_of(v), |_| true);
        assert_eq!(tris.len(), 8);
        assert!(closed(&tris));
        for (t, c) in &tris {
            assert_eq!(*c, 7);
            // Facing away from the voxel's centre.
            let n = (t[1] - t[0]).cross(t[2] - t[0]);
            let centre = (t[0] + t[1] + t[2]) / 3.0;
            assert!(n.dot(centre - Vec3::splat(0.5)) > 0.0);
        }
    }

    #[test]
    fn random_shapes_are_closed_with_colours_kept_apart() {
        let mut v = Volume::new([6, 5, 4]);
        let mut seed = 12345u32;
        for z in 0..4 {
            for y in 0..5 {
                for x in 0..6 {
                    seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
                    if seed >> 16 & 1 == 1 {
                        v.set(x, y, z, 1 + (seed >> 20 & 3) as u8);
                    }
                }
            }
        }
        let tris = triangles(&scene_of(v), |_| true);
        assert!(!tris.is_empty());
        assert!(closed(&tris), "the surface has holes or flipped faces");
        assert!(tris.iter().all(|(_, c)| (1..=4).contains(c)));
    }

    #[test]
    fn ply_counts() {
        let mut v = Volume::new([1, 1, 1]);
        v.set(0, 0, 0, 7);
        let ply = ply(&scene_of(v), &Palette::specification_default(), |_| true, 0.1);
        assert!(ply.contains("element vertex 24\n") && ply.contains("element face 8\n"));
        assert!(ply.trim_end().ends_with("3 21 22 23"));
    }
}
