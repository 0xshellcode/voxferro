//! Voxel volume, meshing, scene graph and command stack.
//!
//! Deliberately free of graphics dependencies: `cargo test -p voxel-core` runs
//! in under a second instead of rebuilding the wgpu stack.

mod atmosphere;
mod command;
mod env_map;
mod mesh;
mod raycast;
mod scene;
mod transform;
mod volume;

pub use atmosphere::{Atmosphere, AtmosphereSettings, PLANET_RADIUS, SKY_SCALE};
pub use command::{Command, History};
pub use env_map::{EnvMap, EnvSampling, HdrError};
pub use mesh::{Face, Mesh, Vertex};
pub use raycast::{raycast, Hit};
pub use scene::{Instance, NodeId, Scene};
pub use transform::Transform;
pub use volume::Volume;
