//! Voxel volume, meshing, scene graph and command stack.
//!
//! Deliberately free of graphics dependencies: `cargo test -p voxel-core` runs
//! in under a second instead of rebuilding the wgpu stack.

mod command;
mod mesh;
mod raycast;
mod scene;
mod transform;
mod volume;

pub use command::{Command, History};
pub use mesh::{Face, Mesh, Vertex};
pub use raycast::{raycast, Hit};
pub use scene::{Instance, NodeId, Scene};
pub use transform::Transform;
pub use volume::Volume;
