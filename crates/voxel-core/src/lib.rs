//! Voxel volume, meshing, scene graph and command stack.
//!
//! Deliberately free of graphics dependencies: `cargo test -p voxel-core` runs
//! in under a second instead of rebuilding the wgpu stack.

mod mesh;
mod volume;

pub use mesh::{Face, Mesh, Vertex};
pub use volume::Volume;
