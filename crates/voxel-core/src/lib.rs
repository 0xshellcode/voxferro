//! Voxel volume, meshing, scene graph and command stack.
//!
//! Deliberately free of graphics dependencies: `cargo test -p voxel-core` runs
//! in under a second instead of rebuilding the wgpu stack.

pub mod anim;
mod atmosphere;
pub mod brush;
mod command;
mod document;
pub mod graph;
mod env_map;
pub mod export;
pub mod formats;
pub mod greedy;
pub mod kernels;
pub mod marching;
mod mesh;
pub mod mesh_cache;
pub mod ops;
mod raycast;
pub mod saved_camera;
pub mod sprites;
mod scene;
mod selection;
mod transform;
pub mod volume;
pub mod voxelize;

pub use brush::{BrushMode, BrushShape};
pub use atmosphere::{Atmosphere, AtmosphereSettings, PLANET_RADIUS, SKY_SCALE};
pub use command::{Command, History};
pub use document::Document;
pub use env_map::{EnvMap, EnvSampling, HdrError};
pub use mesh::{Face, Mesh, Vertex};
pub use raycast::{raycast, Hit};
pub use scene::{BakeError, Baked, Instance, NodeId, Scene, ScenePick, local_cell, world_cell};
pub use selection::{Clipboard, Combine, Selection};
pub use transform::Transform;
pub use volume::Volume;
