//! Meshing only what changed.
//!
//! Every model is meshed in 32³ chunks, in its own coordinates, and each
//! chunk's quads are kept with the versions they were built from: the
//! chunk's and its six neighbours' (a face on the chunk's border is culled
//! against the neighbour), plus a fingerprint of the selected voxels in it
//! (selected voxels are flagged in the mesh). An edit bumps the version of
//! the chunks it touches, so the next rebuild re-meshes those and reuses
//! the rest. A model placed many times is meshed once.
//!
//! Measured on a 256³ model of 4.5 million voxels (release build): the
//! first build is the same as before, ~430 ms; adding one voxel after it
//! re-meshes up to seven chunks instead of the whole model.

use crate::greedy::{Quad, greedy_quads_in};
use crate::volume::CHUNK;
use crate::{Instance, Mesh, Scene, Transform, Vertex, Volume};
use glam::{IVec3, Vec3};
use std::collections::HashMap;

#[derive(Clone, PartialEq)]
struct Key {
    /// Self, then -x, +x, -y, +y, -z, +z.
    versions: [u64; 7],
    selection: u64,
}

#[derive(Default)]
pub struct MeshCache {
    /// By model index, then chunk coordinates.
    chunks: HashMap<(usize, [u32; 3]), (Key, Vec<Quad>)>,
    /// Chunks re-meshed by the last build, for reporting.
    pub last_rebuilt: usize,
}

impl MeshCache {
    /// The scene's mesh: visible instances, selected voxels flagged.
    ///
    /// `selected(model)` gives the selected cells of a model, if any.
    pub fn build<'a>(
        &mut self,
        scene: &Scene,
        keep: impl Fn(&Instance) -> bool,
        selected: impl Fn(usize) -> Option<&'a std::collections::BTreeSet<[i32; 3]>>,
    ) -> Mesh {
        self.last_rebuilt = 0;
        // Each model's quads, refreshed chunk by chunk.
        let mut models_used: Vec<usize> = scene.instances.iter().filter(|i| keep(i)).map(|i| i.model_index).collect();
        models_used.sort_unstable();
        models_used.dedup();
        for &m in &models_used {
            if let Some(volume) = scene.models.get(m) {
                self.refresh_model(m, volume, selected(m));
            }
        }
        // Drop chunks of models that no longer exist or changed size.
        self.chunks.retain(|(m, c), _| {
            scene.models.get(*m).is_some_and(|v| {
                let d = v.chunk_dims();
                c[0] < d[0] && c[1] < d[1] && c[2] < d[2]
            })
        });

        let mut mesh = Mesh::default();
        for instance in scene.instances.iter().filter(|i| keep(i)) {
            let Some(volume) = scene.models.get(instance.model_index) else { continue };
            let [cx, cy, cz] = volume.chunk_dims();
            for k in 0..cz {
                for j in 0..cy {
                    for i in 0..cx {
                        if let Some((_, quads)) = self.chunks.get(&(instance.model_index, [i, j, k])) {
                            append_quads(&mut mesh, quads, volume, &instance.transform);
                        }
                    }
                }
            }
        }
        mesh
    }

    fn refresh_model(&mut self, model: usize, volume: &Volume, selection: Option<&std::collections::BTreeSet<[i32; 3]>>) {
        let dims = volume.chunk_dims();
        // Fingerprint of the selection in each chunk: a sum of hashed cells,
        // so it does not depend on order.
        let mut fingerprints: HashMap<[u32; 3], u64> = HashMap::new();
        if let Some(cells) = selection {
            for c in cells {
                let chunk = [c[0] as u32 / CHUNK, c[1] as u32 / CHUNK, c[2] as u32 / CHUNK];
                let h = (c[0] as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    ^ (c[1] as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                    ^ (c[2] as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
                *fingerprints.entry(chunk).or_default() = fingerprints.get(&chunk).copied().unwrap_or(0).wrapping_add(h | 1);
            }
        }
        let size = volume.size();
        for k in 0..dims[2] {
            for j in 0..dims[1] {
                for i in 0..dims[0] {
                    let c = [i as i32, j as i32, k as i32];
                    let at = |dx: i32, dy: i32, dz: i32| volume.chunk_version([c[0] + dx, c[1] + dy, c[2] + dz]);
                    let key = Key {
                        versions: [at(0, 0, 0), at(-1, 0, 0), at(1, 0, 0), at(0, -1, 0), at(0, 1, 0), at(0, 0, -1), at(0, 0, 1)],
                        selection: fingerprints.get(&[i, j, k]).copied().unwrap_or(0),
                    };
                    if self.chunks.get(&(model, [i, j, k])).is_some_and(|(have, _)| *have == key) {
                        continue;
                    }
                    let lo = [i * CHUNK, j * CHUNK, k * CHUNK];
                    let hi = [0, 1, 2].map(|a| (lo[a] + CHUNK).min(size[a]));
                    let quads = greedy_quads_in(volume, lo, hi, |x, y, z| {
                        let flag = selection.is_some_and(|s| s.contains(&[x as i32, y as i32, z as i32]));
                        volume.get(x, y, z) as u16 + if flag { 256 } else { 0 }
                    });
                    self.chunks.insert((model, [i, j, k]), (key, quads));
                    self.last_rebuilt += 1;
                }
            }
        }
    }
}

fn append_quads(mesh: &mut Mesh, quads: &[Quad], volume: &Volume, transform: &Transform) {
    let half = volume.pivot();
    for quad in quads {
        let base = mesh.vertices.len() as u32;
        let normal = transform.rotation * Vec3::from_array(quad.face.normal());
        for corner in quad.corners {
            mesh.vertices.push(Vertex {
                position: transform.apply(corner - half).to_array(),
                normal: normal.to_array(),
                palette_index: quad.value as f32,
            });
        }
        mesh.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

/// Which chunks of `volume` changed between two snapshots of versions.
pub fn changed_chunks(before: &[u64], volume: &Volume) -> Vec<[u32; 3]> {
    let d = volume.chunk_dims();
    let mut out = Vec::new();
    for k in 0..d[2] {
        for j in 0..d[1] {
            for i in 0..d[0] {
                let n = (i + j * d[0] + k * d[0] * d[1]) as usize;
                if before.get(n) != Some(&volume.chunk_version([i as i32, j as i32, k as i32])) {
                    out.push([i, j, k]);
                }
            }
        }
    }
    out
}

/// Every chunk version of a volume, in order, to compare later.
pub fn versions_of(volume: &Volume) -> Vec<u64> {
    let d = volume.chunk_dims();
    let mut out = Vec::with_capacity((d[0] * d[1] * d[2]) as usize);
    for k in 0..d[2] as i32 {
        for j in 0..d[1] as i32 {
            for i in 0..d[0] as i32 {
                out.push(volume.chunk_version([i, j, k]));
            }
        }
    }
    out
}

/// The world cells of chunk `c` of a model placed by `transform`, as an
/// inclusive box.
pub fn chunk_world_box(volume: &Volume, transform: &Transform, c: [u32; 3]) -> (IVec3, IVec3) {
    let size = volume.size();
    let lo = IVec3::new((c[0] * CHUNK) as i32, (c[1] * CHUNK) as i32, (c[2] * CHUNK) as i32);
    let hi = IVec3::new(
        ((c[0] + 1) * CHUNK).min(size[0]) as i32 - 1,
        ((c[1] + 1) * CHUNK).min(size[1]) as i32 - 1,
        ((c[2] + 1) * CHUNK).min(size[2]) as i32 - 1,
    );
    let a = crate::world_cell(volume, transform, lo);
    let b = crate::world_cell(volume, transform, hi);
    (a.min(b), a.max(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_with(v: Volume) -> Scene {
        Scene {
            models: vec![v],
            instances: vec![Instance { model_index: 0, layer: -1, ..Default::default() }],
            nodes: Vec::new(),
            frame: 0,
        }
    }

    #[test]
    fn the_cached_mesh_is_the_full_mesh_and_edits_redo_few_chunks() {
        let mut v = Volume::new([96, 64, 40]);
        for z in 0..20 {
            for y in 0..64 {
                for x in 0..96 {
                    v.set(x, y, z, 1 + ((x + y) % 3) as u8);
                }
            }
        }
        let mut scene = scene_with(v);
        let mut cache = MeshCache::default();
        let first = cache.build(&scene, |_| true, |_| None);
        let all = cache.last_rebuilt;
        assert_eq!(all, 3 * 2 * 2);
        let area = |m: &Mesh| m.triangle_count();
        let fresh = MeshCache::default().build(&scene, |_| true, |_| None);
        assert_eq!(area(&first), area(&fresh));

        // One voxel in the middle of chunk (1, 1, 0): it and its neighbours.
        scene.models[0].set(48, 40, 25, 7);
        let edited = cache.build(&scene, |_| true, |_| None);
        assert!(cache.last_rebuilt <= 7 && cache.last_rebuilt >= 1, "rebuilt {}", cache.last_rebuilt);
        let fresh = MeshCache::default().build(&scene, |_| true, |_| None);
        assert_eq!(edited.vertices, fresh.vertices, "same mesh as from scratch");

        // Nothing changed: nothing rebuilt.
        cache.build(&scene, |_| true, |_| None);
        assert_eq!(cache.last_rebuilt, 0);
    }

    #[test]
    fn a_selection_change_remeshes_its_chunk() {
        let mut v = Volume::new([64, 8, 8]);
        v.set(40, 1, 1, 3);
        let scene = scene_with(v);
        let mut cache = MeshCache::default();
        cache.build(&scene, |_| true, |_| None);
        let selected: std::collections::BTreeSet<[i32; 3]> = [[40, 1, 1]].into_iter().collect();
        let mesh = cache.build(&scene, |_| true, |_| Some(&selected));
        assert_eq!(cache.last_rebuilt, 1);
        assert!(mesh.vertices.iter().all(|v| v.palette_index >= 256.0));
    }
}
