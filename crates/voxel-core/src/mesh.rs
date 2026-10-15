//! Turns a voxel volume into a triangle mesh.
//!
//! Only faces with an empty neighbour are emitted. For a solid 64³ block that
//! is 24 576 faces instead of 1.5 million — the difference between a viewer
//! that runs and one that does not.
//!
//! the reference editor does the same thing with six template instantiations of
//! `the reference renderer<>`, one per face direction.

use crate::Volume;

/// Which way a face points. The order fixes the face index used by picking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Face {
    NegX = 0,
    PosX = 1,
    NegY = 2,
    PosY = 3,
    NegZ = 4,
    PosZ = 5,
}

impl Face {
    pub const ALL: [Face; 6] = [
        Face::NegX,
        Face::PosX,
        Face::NegY,
        Face::PosY,
        Face::NegZ,
        Face::PosZ,
    ];

    /// Unit step from a voxel towards this face's neighbour.
    pub fn offset(self) -> [i32; 3] {
        match self {
            Face::NegX => [-1, 0, 0],
            Face::PosX => [1, 0, 0],
            Face::NegY => [0, -1, 0],
            Face::PosY => [0, 1, 0],
            Face::NegZ => [0, 0, -1],
            Face::PosZ => [0, 0, 1],
        }
    }

    pub fn normal(self) -> [f32; 3] {
        let o = self.offset();
        [o[0] as f32, o[1] as f32, o[2] as f32]
    }

    /// The four corners of this face on a unit cube, counter-clockwise when
    /// seen from outside, so a single winding order covers all six faces.
    fn corners(self) -> [[f32; 3]; 4] {
        match self {
            Face::NegX => [
                [0.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 1.0, 0.0],
            ],
            Face::PosX => [
                [1.0, 0.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 1.0, 1.0],
                [1.0, 0.0, 1.0],
            ],
            Face::NegY => [
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            Face::PosY => [
                [0.0, 1.0, 0.0],
                [0.0, 1.0, 1.0],
                [1.0, 1.0, 1.0],
                [1.0, 1.0, 0.0],
            ],
            Face::NegZ => [
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
            ],
            Face::PosZ => [
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 1.0],
                [1.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
            ],
        }
    }
}

/// One mesh vertex.
///
/// The palette index rides along as a float attribute rather than a resolved
/// colour: the shader looks it up, so recolouring the palette does not require
/// rebuilding the mesh. the reference editor smuggles the same value in the vertex's W
/// coordinate.
/// `Pod` + `Zeroable` let the vertex buffer be handed to the GPU as raw bytes
/// with no per-vertex conversion. It requires a fixed `repr(C)` layout and no
/// padding, which this struct satisfies: seven consecutive f32s.
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub palette_index: f32,
}

/// A mesh ready to upload to the GPU.
#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Emits every face of every filled voxel whose neighbour is empty.
    pub fn from_volume(volume: &Volume) -> Self {
        let [sx, sy, sz] = volume.size();
        let mut mesh = Self::default();

        for z in 0..sz {
            for y in 0..sy {
                for x in 0..sx {
                    let index = volume.get(x, y, z);
                    if index == 0 {
                        continue;
                    }
                    for face in Face::ALL {
                        let [ox, oy, oz] = face.offset();
                        let neighbour =
                            volume.get_i(x as i32 + ox, y as i32 + oy, z as i32 + oz);
                        if neighbour == 0 {
                            mesh.push_face([x as f32, y as f32, z as f32], face, index);
                        }
                    }
                }
            }
        }
        mesh
    }

    fn push_face(&mut self, origin: [f32; 3], face: Face, palette_index: u8) {
        let base = self.vertices.len() as u32;
        let normal = face.normal();
        for corner in face.corners() {
            self.vertices.push(Vertex {
                position: [
                    origin[0] + corner[0],
                    origin[1] + corner[1],
                    origin[2] + corner[2],
                ],
                normal,
                palette_index: palette_index as f32,
            });
        }
        // Two triangles per quad, sharing the 0-2 diagonal.
        self.indices
            .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_volume_makes_no_mesh() {
        let mesh = Mesh::from_volume(&Volume::new([4, 4, 4]));
        assert!(mesh.is_empty());
    }

    #[test]
    fn a_lone_voxel_has_six_faces() {
        let mut v = Volume::new([3, 3, 3]);
        v.set(1, 1, 1, 7);
        let mesh = Mesh::from_volume(&v);
        assert_eq!(mesh.triangle_count(), 12, "6 faces * 2 triangles");
        assert_eq!(mesh.vertices.len(), 24, "6 faces * 4 corners");
        assert!(mesh.vertices.iter().all(|vert| vert.palette_index == 7.0));
    }

    #[test]
    fn touching_voxels_hide_the_shared_face() {
        let mut v = Volume::new([4, 1, 1]);
        v.set(0, 0, 0, 1);
        v.set(1, 0, 0, 1);
        // Two cubes side by side: 12 faces total, minus the two that touch.
        let mesh = Mesh::from_volume(&v);
        assert_eq!(mesh.triangle_count(), 10 * 2);
    }

    #[test]
    fn a_solid_block_only_meshes_its_shell() {
        let n = 8;
        let mut v = Volume::new([n, n, n]);
        for z in 0..n {
            for y in 0..n {
                for x in 0..n {
                    v.set(x, y, z, 1);
                }
            }
        }
        // Only the outer shell is visible: 6 sides of n*n faces.
        let mesh = Mesh::from_volume(&v);
        let expected_faces = 6 * n * n;
        assert_eq!(mesh.triangle_count(), (expected_faces * 2) as usize);
    }
}
