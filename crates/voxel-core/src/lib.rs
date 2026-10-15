//! Voxel volume, meshing, scene graph and command stack.
//!
//! Deliberately free of graphics dependencies: `cargo test -p voxel-core` runs
//! in under a second instead of rebuilding the wgpu stack.

mod mesh;
mod scene;
mod transform;
mod volume;

pub use mesh::{Face, Mesh, Vertex};
pub use scene::{Instance, NodeId, Scene};
pub use transform::Transform;
pub use volume::Volume;
