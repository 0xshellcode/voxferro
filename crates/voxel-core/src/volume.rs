//! Dense voxel volume.

use std::sync::atomic::{AtomicU64, Ordering};
use vox_format::Model;

/// Side of the chunks a volume is versioned in.
pub const CHUNK: u32 = 32;

/// Source of chunk versions. Global, so a version is never reused: two
/// chunks with the same version hold the same voxels (one is a clone of the
/// other, untouched since), whatever volume they are in.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

fn fresh_version() -> u64 {
    NEXT_VERSION.fetch_add(1, Ordering::Relaxed)
}

/// A dense voxel grid. Each cell holds a palette index, where 0 means empty.
///
/// Dense rather than sparse: the editor caps volumes at 256³, so the worst
/// case is 16 MB — cheap enough that constant-time random access is the better
/// trade. Sparsity belongs in the GPU-side acceleration structure, not here.
///
/// Each 32³ chunk carries a version that changes whenever one of its voxels
/// does. Caches of derived data -- the mesh, the traced grid -- compare
/// versions to redo only the chunks an edit touched, whatever path the edit
/// took (brush, console, kernel, undo).
#[derive(Debug, Clone, Default)]
pub struct Volume {
    size: [u32; 3],
    voxels: Vec<u8>,
    versions: Vec<u64>,
}

/// Equal voxels, whatever the versions.
impl PartialEq for Volume {
    fn eq(&self, other: &Self) -> bool {
        self.size == other.size && self.voxels == other.voxels
    }
}

impl Volume {
    pub fn new(size: [u32; 3]) -> Self {
        let count = (size[0] as usize) * (size[1] as usize) * (size[2] as usize);
        let chunks = Self::chunk_dims_of(size);
        let chunk_count = (chunks[0] * chunks[1] * chunks[2]) as usize;
        Self {
            size,
            voxels: vec![0; count],
            versions: (0..chunk_count).map(|_| fresh_version()).collect(),
        }
    }

    /// A volume from its cells, x fastest, then y, then z.
    pub fn from_cells(size: [u32; 3], voxels: Vec<u8>) -> Self {
        let mut out = Self::new(size);
        assert_eq!(voxels.len(), out.voxels.len(), "cells for a {size:?} volume");
        out.voxels = voxels;
        out
    }

    fn chunk_dims_of(size: [u32; 3]) -> [u32; 3] {
        size.map(|s| s.div_ceil(CHUNK))
    }

    /// How many chunks along each axis.
    pub fn chunk_dims(&self) -> [u32; 3] {
        Self::chunk_dims_of(self.size)
    }

    /// The version of chunk `c` (in chunk coordinates), or 0 outside.
    pub fn chunk_version(&self, c: [i32; 3]) -> u64 {
        let d = self.chunk_dims();
        if c.iter().zip(d).any(|(&c, d)| c < 0 || c as u32 >= d) {
            return 0;
        }
        self.versions[(c[0] as u32 + c[1] as u32 * d[0] + c[2] as u32 * d[0] * d[1]) as usize]
    }

    /// Builds a volume from a parsed model's sparse voxel list.
    pub fn from_model(model: &Model) -> Self {
        let mut out = Self::new(model.size);
        for v in &model.voxels {
            out.set(v.x as u32, v.y as u32, v.z as u32, v.index);
        }
        out
    }

    pub fn size(&self) -> [u32; 3] {
        self.size
    }

    /// The point a model's transform places, in voxel coordinates.
    ///
    /// Integer division, not `size / 2.0`: every model in the sample corpus
    /// is written with `_t.z == size.z / 2` (a 3-high model at z = 1), which
    /// only sits it on the ground with an integer pivot. A fractional pivot
    /// would push every odd-sized model half a voxel off the world grid.
    pub fn pivot(&self) -> glam::Vec3 {
        glam::Vec3::new(
            (self.size[0] / 2) as f32,
            (self.size[1] / 2) as f32,
            (self.size[2] / 2) as f32,
        )
    }

    fn index(&self, x: u32, y: u32, z: u32) -> Option<usize> {
        if x >= self.size[0] || y >= self.size[1] || z >= self.size[2] {
            return None;
        }
        Some((x + y * self.size[0] + z * self.size[0] * self.size[1]) as usize)
    }

    /// Palette index at a coordinate; 0 for empty or out of bounds.
    ///
    /// Out-of-bounds reads answer "empty" rather than erroring, because the
    /// mesher queries neighbours across the volume edge on every face.
    pub fn get(&self, x: u32, y: u32, z: u32) -> u8 {
        self.index(x, y, z).map_or(0, |i| self.voxels[i])
    }

    /// Signed variant, for callers walking a ray that may leave the volume.
    pub fn get_i(&self, x: i32, y: i32, z: i32) -> u8 {
        if x < 0 || y < 0 || z < 0 {
            return 0;
        }
        self.get(x as u32, y as u32, z as u32)
    }

    pub fn set(&mut self, x: u32, y: u32, z: u32, value: u8) {
        if let Some(i) = self.index(x, y, z) {
            if self.voxels[i] != value {
                self.voxels[i] = value;
                let d = self.chunk_dims();
                let (cx, cy, cz) = (x / CHUNK, y / CHUNK, z / CHUNK);
                self.versions[(cx + cy * d[0] + cz * d[0] * d[1]) as usize] = fresh_version();
            }
        }
    }

    pub fn is_empty_at(&self, x: u32, y: u32, z: u32) -> bool {
        self.get(x, y, z) == 0
    }

    /// Converts back to the sparse list a `.vox` file stores.
    ///
    /// Coordinates are `u8` in the format, so a volume may not exceed 256 on
    /// any axis — which is also the editor's own limit.
    pub fn to_model(&self) -> vox_format::Model {
        let [sx, sy, sz] = self.size;
        let mut voxels = Vec::with_capacity(self.filled_count());
        for z in 0..sz.min(256) {
            for y in 0..sy.min(256) {
                for x in 0..sx.min(256) {
                    let index = self.get(x, y, z);
                    if index != 0 {
                        voxels.push(vox_format::Voxel {
                            x: x as u8,
                            y: y as u8,
                            z: z as u8,
                            index,
                        });
                    }
                }
            }
        }
        vox_format::Model { size: self.size, voxels }
    }

    /// Inclusive bounding box of the non-empty voxels, or `None` if empty.
    ///
    /// A model rarely fills its declared volume, so framing the declared size
    /// leaves the subject small and off-centre. Framing this instead fills the
    /// view with what is actually there.
    pub fn occupied_bounds(&self) -> Option<([u32; 3], [u32; 3])> {
        let [sx, sy, sz] = self.size;
        let mut min = [u32::MAX; 3];
        let mut max = [0u32; 3];
        let mut any = false;

        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    if self.get(x, y, z) == 0 {
                        continue;
                    }
                    any = true;
                    let p = [x, y, z];
                    for i in 0..3 {
                        min[i] = min[i].min(p[i]);
                        max[i] = max[i].max(p[i]);
                    }
                }
            }
        }
        any.then_some((min, max))
    }

    /// How many voxels use each palette index (slot 0 counts empty cells).
    pub fn histogram(&self) -> [u64; 256] {
        let mut counts = [0u64; 256];
        for &v in &self.voxels {
            counts[v as usize] += 1;
        }
        counts
    }

    /// Number of non-empty voxels.
    pub fn filled_count(&self) -> usize {
        self.voxels.iter().filter(|&&v| v != 0).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_round_trips_a_voxel() {
        let mut v = Volume::new([3, 3, 3]);
        assert!(v.is_empty_at(1, 1, 1));
        v.set(1, 1, 1, 42);
        assert_eq!(v.get(1, 1, 1), 42);
        assert_eq!(v.filled_count(), 1);
    }

    #[test]
    fn model_round_trips_through_a_volume() {
        let mut v = Volume::new([4, 5, 6]);
        v.set(1, 2, 3, 9);
        v.set(0, 0, 0, 1);

        let model = v.to_model();
        assert_eq!(model.size, [4, 5, 6]);
        assert_eq!(model.voxels.len(), 2);

        let back = Volume::from_model(&model);
        assert_eq!(back.get(1, 2, 3), 9);
        assert_eq!(back.get(0, 0, 0), 1);
        assert_eq!(back.filled_count(), 2);
    }

    #[test]
    fn pivot_uses_integer_division() {
        assert_eq!(Volume::new([3, 3, 3]).pivot(), glam::Vec3::new(1.0, 1.0, 1.0));
        assert_eq!(Volume::new([15, 34, 13]).pivot(), glam::Vec3::new(7.0, 17.0, 6.0));
    }

    #[test]
    fn bounds_cover_only_filled_voxels() {
        let mut v = Volume::new([10, 10, 10]);
        assert_eq!(v.occupied_bounds(), None);
        v.set(2, 3, 4, 1);
        v.set(6, 7, 8, 1);
        assert_eq!(v.occupied_bounds(), Some(([2, 3, 4], [6, 7, 8])));
    }

    #[test]
    fn out_of_bounds_reads_as_empty() {
        let v = Volume::new([2, 2, 2]);
        assert_eq!(v.get(9, 0, 0), 0);
        assert!(v.is_empty_at(0, 0, 9));
        assert_eq!(v.get_i(-1, 0, 0), 0);
    }

    #[test]
    fn chunk_versions_follow_edits_and_clones() {
        let mut v = Volume::new([40, 8, 8]);
        let (a, b) = (v.chunk_version([0, 0, 0]), v.chunk_version([1, 0, 0]));
        assert_ne!(a, b, "every chunk starts distinct");
        v.set(35, 0, 0, 3);
        assert_eq!(v.chunk_version([0, 0, 0]), a, "the other chunk is untouched");
        assert_ne!(v.chunk_version([1, 0, 0]), b);
        let c = v.chunk_version([1, 0, 0]);
        v.set(35, 0, 0, 3);
        assert_eq!(v.chunk_version([1, 0, 0]), c, "rewriting the same value is no change");
        let copy = v.clone();
        assert_eq!(copy.chunk_version([1, 0, 0]), c, "a clone shares versions");
        assert_eq!(copy, v);
    }
}

