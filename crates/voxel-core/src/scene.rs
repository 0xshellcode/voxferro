//! The scene graph: `nTRN` → `nGRP` → `nSHP` turned into a walkable tree.

use crate::{Transform, Volume};
use vox_format::{SceneNode, VoxFile};

/// Index of a node in the arena.
///
/// A newtype rather than a bare `u32`: it cannot be confused with a model
/// index or a layer number at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// One placed model: which model, where, and on which layer.
#[derive(Debug, Clone)]
pub struct Instance {
    pub model_index: usize,
    pub transform: Transform,
    pub layer: i32,
}

/// A scene flattened into the list of model placements it describes.
///
/// The file stores nodes as a flat list referring to each other by id. Rather
/// than rebuilding a pointer tree, we walk it once with an explicit stack and
/// keep the result: nothing downstream needs the intermediate shape.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub instances: Vec<Instance>,
    pub models: Vec<Volume>,
}

impl Scene {
    /// Builds the scene at animation frame 0.
    pub fn from_file(file: &VoxFile) -> Self {
        Self::from_file_at_frame(file, 0)
    }

    /// Builds the scene at a given animation frame.
    ///
    /// A shape node with several models is not several models placed at once:
    /// it is one model keyed by animation frame, each entry tagged with `_f`.
    /// Drawing them all would stack every frame of the animation on top of
    /// itself.
    pub fn from_file_at_frame(file: &VoxFile, frame: u32) -> Self {
        let models: Vec<Volume> = file.models.iter().map(Volume::from_model).collect();

        // Index nodes by id so children resolve in constant time.
        let mut by_id = std::collections::HashMap::new();
        for node in &file.nodes {
            by_id.insert(node.id(), node);
        }

        let mut instances = Vec::new();
        // Walk from the root transform, carrying the accumulated transform.
        // An explicit stack rather than recursion: a malformed file could
        // nest arbitrarily deep, and a stack overflow is not a nice failure.
        let root = file.nodes.first().map(|n| n.id()).unwrap_or(0);
        let mut stack = vec![(root, Transform::default(), -1i32, 0u32)];
        const MAX_DEPTH: u32 = 1024;

        while let Some((id, parent_transform, layer, depth)) = stack.pop() {
            if depth > MAX_DEPTH {
                continue;
            }
            let Some(node) = by_id.get(&id) else { continue };

            match node {
                SceneNode::Transform(t) => {
                    // Frame 0 only: animation playback would pick another.
                    let local = Transform::from_parts(t.rotation_bits(0), t.translation(0));
                    let combined = local.then(&parent_transform);
                    let layer = if t.layer >= 0 { t.layer } else { layer };
                    stack.push((t.child, combined, layer, depth + 1));
                }
                SceneNode::Group(g) => {
                    for child in &g.children {
                        stack.push((*child, parent_transform, layer, depth + 1));
                    }
                }
                SceneNode::Shape(s) => {
                    // Pick the entry for this frame; fall back to the first
                    // when the shape is not animated.
                    let chosen = s
                        .models
                        .iter()
                        .find(|m| {
                            m.attributes
                                .get("_f")
                                .and_then(|v| v.parse::<u32>().ok())
                                .is_some_and(|f| f == frame)
                        })
                        .or_else(|| s.models.first());

                    if let Some(m) = chosen {
                        let index = m.model_id as usize;
                        if index < models.len() {
                            instances.push(Instance {
                                model_index: index,
                                transform: parent_transform,
                                layer,
                            });
                        }
                    }
                }
            }
        }

        // A file with voxels but no usable graph still deserves to render.
        if instances.is_empty() && !models.is_empty() {
            instances.push(Instance {
                model_index: 0,
                transform: Transform::default(),
                layer: -1,
            });
        }

        Self { instances, models }
    }

    /// How many animation frames the file declares, from the largest shape.
    pub fn frame_count(file: &VoxFile) -> u32 {
        file.nodes
            .iter()
            .filter_map(|n| match n {
                SceneNode::Shape(s) => Some(s.models.len() as u32),
                _ => None,
            })
            .max()
            .unwrap_or(1)
            .max(1)
    }

    pub fn total_voxels(&self) -> usize {
        self.instances
            .iter()
            .filter_map(|i| self.models.get(i.model_index))
            .map(|m| m.filled_count())
            .sum()
    }
}
