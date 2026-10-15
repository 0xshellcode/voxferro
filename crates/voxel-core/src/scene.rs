//! The scene graph: `nTRN` → `nGRP` → `nSHP` turned into a walkable tree.

use crate::{Hit, Transform, Volume, raycast};
use glam::{IVec3, Vec3};
use vox_format::{SceneNode, VoxFile};

/// Index of a node in the arena.
///
/// A newtype rather than a bare `u32`: it cannot be confused with a model
/// index or a layer number at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// One placed model: which model, where, and on which layer.
#[derive(Debug, Clone, Default)]
pub struct Instance {
    pub model_index: usize,
    /// World transform: the whole chain of transform nodes above it.
    pub transform: Transform,
    pub layer: i32,
    /// The `nTRN` directly above the shape, the one that moves this
    /// instance alone. `None` when the shape hangs straight off a group, or
    /// when there is no graph.
    pub node: Option<i32>,
    /// The world transform of that node's parent, so a new world placement
    /// can be turned back into the node's own local one.
    pub parent: Transform,
}

/// The scene graph as stored, plus the list of model placements it
/// describes.
///
/// The file stores nodes as a flat list referring to each other by id. The
/// nodes stay the source of truth -- edits change them and saving writes them
/// back, groups, names and animation frames included -- and `instances` is
/// their flattened form, rebuilt after every change with an explicit-stack
/// walk rather than a pointer tree.
#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub instances: Vec<Instance>,
    pub models: Vec<Volume>,
    pub nodes: Vec<SceneNode>,
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
        // A file without a graph gets the one a save would write, so the two
        // agree on where its models sit and edits have nodes to change.
        let nodes = if file.nodes.is_empty() {
            vox_format::minimal_graph(&file.models)
        } else {
            file.nodes.clone()
        };
        let instances = flatten(&nodes, models.len(), frame);
        Self { instances, models, nodes }
    }

    /// Recomputes `instances` after the nodes or the model list changed.
    pub fn rebuild(&mut self) {
        self.instances = flatten(&self.nodes, self.models.len(), 0);
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

    /// Every visible instance written into one world-aligned grid.
    ///
    /// This is what the path tracer walks. Rotations are restricted to the
    /// 24 axis-aligned orientations and pivots are integers, so each voxel
    /// centre lands exactly on a world cell centre: baking loses nothing.
    /// Where instances overlap, the later one wins.
    ///
    /// `None` when nothing is visible or the grid would exceed `max_cells`
    /// (instances placed far apart make a mostly empty, very large box).
    pub fn bake(&self, keep: impl Fn(&Instance) -> bool, max_cells: u64) -> Result<Baked, BakeError> {
        let visible: Vec<&Instance> = self
            .instances
            .iter()
            .filter(|i| keep(i) && self.models.get(i.model_index).is_some())
            .collect();

        let mut bounds: Option<(IVec3, IVec3)> = None;
        for instance in &visible {
            let volume = &self.models[instance.model_index];
            let Some((lo, hi)) = volume.occupied_bounds() else { continue };
            for corner in 0..8 {
                let pick = |bit: u32, axis: usize| if corner & bit != 0 { hi[axis] } else { lo[axis] };
                let local = IVec3::new(pick(1, 0) as i32, pick(2, 1) as i32, pick(4, 2) as i32);
                let world = world_cell(volume, &instance.transform, local);
                bounds = Some(match bounds {
                    None => (world, world),
                    Some((a, b)) => (a.min(world), b.max(world)),
                });
            }
        }
        let Some((min, max)) = bounds else {
            return Err(BakeError::Empty);
        };

        let size = (max - min + IVec3::ONE).as_uvec3();
        let cells = size.x as u64 * size.y as u64 * size.z as u64;
        if cells > max_cells {
            return Err(BakeError::TooLarge(size.to_array()));
        }

        let mut grid = Volume::new(size.to_array());
        for instance in &visible {
            let volume = &self.models[instance.model_index];
            let [sx, sy, sz] = volume.size();
            for z in 0..sz {
                for y in 0..sy {
                    for x in 0..sx {
                        let index = volume.get(x, y, z);
                        if index == 0 {
                            continue;
                        }
                        let local = IVec3::new(x as i32, y as i32, z as i32);
                        let g = world_cell(volume, &instance.transform, local) - min;
                        grid.set(g.x as u32, g.y as u32, g.z as u32, index);
                    }
                }
            }
        }
        Ok(Baked { volume: grid, origin: min })
    }

    /// The nearest voxel a world ray hits across every instance `keep`
    /// accepts, as the instance index plus the hit in that model's own
    /// coordinates.
    pub fn pick(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
        keep: impl Fn(&Instance) -> bool,
    ) -> Option<ScenePick> {
        let mut best: Option<ScenePick> = None;
        for (i, instance) in self.instances.iter().enumerate() {
            if !keep(instance) {
                continue;
            }
            let Some(volume) = self.models.get(instance.model_index) else { continue };
            let local_origin = instance.transform.inverse_apply(origin) + volume.pivot();
            // Rotations are orthonormal, so distances agree in both spaces.
            let local_dir = instance.transform.inverse_rotate(direction).normalize_or_zero();
            let Some(hit) = raycast(volume, local_origin, local_dir, max_distance) else {
                continue;
            };
            let distance = entry_distance(local_origin, local_dir, hit.voxel);
            if best.as_ref().is_none_or(|b| distance < b.distance) {
                best = Some(ScenePick { instance: i, hit, distance });
            }
        }
        best
    }

    /// World bounds of every instance's declared box, filled or not: what
    /// to frame when the models are still empty.
    pub fn declared_bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut bounds: Option<(Vec3, Vec3)> = None;
        for instance in &self.instances {
            let Some(volume) = self.models.get(instance.model_index) else { continue };
            let size = Vec3::from_array(volume.size().map(|s| s as f32));
            for corner in [Vec3::ZERO, size] {
                let p = instance.transform.apply(corner - volume.pivot());
                bounds = Some(match bounds {
                    None => (p, p),
                    Some((a, b)) => (a.min(p), b.max(p)),
                });
            }
        }
        bounds
    }

    pub fn total_voxels(&self) -> usize {
        self.instances
            .iter()
            .filter_map(|i| self.models.get(i.model_index))
            .map(|m| m.filled_count())
            .sum()
    }
}

/// Walks the graph from the root, carrying the accumulated transform.
///
/// A shape node with several models is not several models placed at once: it
/// is one model keyed by animation frame, each entry tagged with `_f`.
/// Drawing them all would stack every frame of the animation on top of
/// itself.
fn flatten(nodes: &[SceneNode], model_count: usize, frame: u32) -> Vec<Instance> {
    // Index nodes by id so children resolve in constant time.
    let by_id: std::collections::HashMap<i32, &SceneNode> = nodes.iter().map(|n| (n.id(), n)).collect();

    let mut instances = Vec::new();
    // An explicit stack rather than recursion: a malformed file could nest
    // arbitrarily deep, and a stack overflow is not a nice failure. Each
    // entry carries the nearest transform node and its parent's transform.
    struct Step {
        id: i32,
        transform: Transform,
        layer: i32,
        owner: Option<(i32, Transform)>,
        depth: u32,
    }
    let root = nodes.first().map(|n| n.id()).unwrap_or(0);
    let mut stack = vec![Step { id: root, transform: Transform::default(), layer: -1, owner: None, depth: 0 }];
    const MAX_DEPTH: u32 = 1024;

    while let Some(step) = stack.pop() {
        if step.depth > MAX_DEPTH {
            continue;
        }
        let Some(node) = by_id.get(&step.id) else { continue };

        match node {
            SceneNode::Transform(t) => {
                // Frame 0 only: animation playback would pick another.
                let local = Transform::from_parts(t.rotation_bits(0), t.translation(0));
                stack.push(Step {
                    id: t.child,
                    transform: local.then(&step.transform),
                    layer: if t.layer >= 0 { t.layer } else { step.layer },
                    owner: Some((t.id, step.transform)),
                    depth: step.depth + 1,
                });
            }
            SceneNode::Group(g) => {
                // Pushed in reverse so they pop, and list, in file order.
                for child in g.children.iter().rev() {
                    stack.push(Step {
                        id: *child,
                        transform: step.transform,
                        layer: step.layer,
                        owner: None,
                        depth: step.depth + 1,
                    });
                }
            }
            SceneNode::Shape(s) => {
                // Pick the entry for this frame; fall back to the first when
                // the shape is not animated.
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
                    if index < model_count {
                        instances.push(Instance {
                            model_index: index,
                            transform: step.transform,
                            layer: step.layer,
                            node: step.owner.map(|(id, _)| id),
                            parent: step.owner.map(|(_, p)| p).unwrap_or_default(),
                        });
                    }
                }
            }
        }
    }

    // A file with voxels but no usable graph still deserves to render.
    if instances.is_empty() && model_count > 0 {
        instances.push(Instance { model_index: 0, layer: -1, ..Default::default() });
    }
    instances
}

/// A model voxel's world cell under a transform.
///
/// The voxel centre relative to the pivot always has .5 fractions, and an
/// axis-aligned rotation only permutes and negates them, so `floor` of the
/// placed centre is exact.
pub fn world_cell(volume: &Volume, transform: &Transform, local: IVec3) -> IVec3 {
    let centre = local.as_vec3() + Vec3::splat(0.5) - volume.pivot();
    transform.apply(centre).floor().as_ivec3()
}

/// The inverse of [`world_cell`]: which voxel of the model covers a world
/// cell. May fall outside the model's box.
pub fn local_cell(volume: &Volume, transform: &Transform, world: IVec3) -> IVec3 {
    let centre = transform.inverse_apply(world.as_vec3() + Vec3::splat(0.5)) + volume.pivot();
    centre.floor().as_ivec3()
}

/// Distance along a ray to where it enters a unit cell.
fn entry_distance(origin: Vec3, direction: Vec3, cell: IVec3) -> f32 {
    let lo = cell.as_vec3();
    let hi = lo + Vec3::ONE;
    let mut t_enter = 0.0f32;
    for axis in 0..3 {
        let d = direction[axis];
        if d.abs() < 1e-12 {
            continue;
        }
        let (a, b) = ((lo[axis] - origin[axis]) / d, (hi[axis] - origin[axis]) / d);
        t_enter = t_enter.max(a.min(b));
    }
    t_enter
}

/// The scene as one grid: `volume` cell (0,0,0) is world cell `origin`.
#[derive(Debug, Clone)]
pub struct Baked {
    pub volume: Volume,
    pub origin: IVec3,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BakeError {
    /// No visible instance holds a voxel.
    Empty,
    /// The bounding box of the visible instances, too big to allocate.
    TooLarge([u32; 3]),
}

impl std::fmt::Display for BakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "nothing visible to trace"),
            Self::TooLarge([x, y, z]) => write!(f, "scene spans {x}x{y}x{z} voxels, too large to trace"),
        }
    }
}

/// Result of [`Scene::pick`].
#[derive(Debug, Clone, Copy)]
pub struct ScenePick {
    pub instance: usize,
    pub hit: Hit,
    pub distance: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_of(placements: &[([i32; 3], u8)]) -> Scene {
        let mut v = Volume::new([3, 3, 3]);
        v.set(0, 0, 0, 5);
        v.set(2, 1, 0, 6);
        Scene {
            models: vec![v],
            instances: placements
                .iter()
                .map(|&(t, bits)| Instance {
                    model_index: 0,
                    transform: Transform::from_parts(bits, t),
                    layer: -1,
                    ..Default::default()
                })
                .collect(),
            nodes: Vec::new(),
        }
    }

    #[test]
    fn baking_places_every_instance() {
        let scene = scene_of(&[([0, 0, 1], 0b0000_0100), ([10, 0, 1], 0b0000_0100)]);
        let baked = scene.bake(|_| true, u64::MAX).unwrap();
        assert_eq!(baked.volume.filled_count(), 4, "two voxels per instance");
        // Instance 1's voxel (0,0,0) sits at world (10-1, -1, 0).
        let g = IVec3::new(9, -1, 0) - baked.origin;
        assert_eq!(baked.volume.get_i(g.x, g.y, g.z), 5);
    }

    #[test]
    fn baking_applies_rotation() {
        // 0b0001_0001: row 0 takes -column 1, row 1 takes column 0: a
        // quarter turn about z.
        let bits = 0b0001_0001;
        let scene = scene_of(&[([0, 0, 1], bits)]);
        let baked = scene.bake(|_| true, u64::MAX).unwrap();
        let t = scene.instances[0].transform;
        let v = &scene.models[0];
        for (local, index) in [(IVec3::new(0, 0, 0), 5), (IVec3::new(2, 1, 0), 6)] {
            let g = world_cell(v, &t, local) - baked.origin;
            assert_eq!(baked.volume.get_i(g.x, g.y, g.z), index);
            assert_eq!(local_cell(v, &t, world_cell(v, &t, local)), local, "round trip");
        }
        // The baked voxel agrees with the mesher: its centre is the
        // rotated, placed voxel centre.
        let centre = t.apply(Vec3::new(2.5, 1.5, 0.5) - v.pivot());
        assert_eq!(world_cell(v, &t, IVec3::new(2, 1, 0)), centre.floor().as_ivec3());
    }

    #[test]
    fn baking_respects_the_filter_and_the_size_limit() {
        let scene = scene_of(&[([0, 0, 1], 0b0000_0100), ([1000, 0, 1], 0b0000_0100)]);
        assert!(matches!(scene.bake(|_| true, 1_000), Err(BakeError::TooLarge(_))));
        let first = scene.bake(|i| i.transform.translation.x == 0, 1_000).unwrap();
        assert_eq!(first.volume.filled_count(), 2);
        assert_eq!(scene.bake(|_| false, 1_000).unwrap_err(), BakeError::Empty);
    }

    #[test]
    fn picking_returns_the_nearest_instance() {
        let scene = scene_of(&[([0, 10, 1], 0b0000_0100), ([0, 5, 1], 0b0000_0100)]);
        // From -y looking +y along the row of voxel (0,0,0) of each copy.
        let origin = Vec3::new(-0.5, -20.0, 0.5);
        let pick = scene.pick(origin, Vec3::Y, 1000.0, |_| true).unwrap();
        assert_eq!(pick.instance, 1, "the copy at y = 5 is in front");
        assert_eq!(pick.hit.voxel, IVec3::new(0, 0, 0));
        // Voxel (0,0,0) minus pivot (1,1,1) plus y = 5 is world cell y = 4.
        assert!((pick.distance - 24.0).abs() < 1e-4, "enters at y = 4: {}", pick.distance);

        let hidden = scene.pick(origin, Vec3::Y, 1000.0, |i| i.transform.translation.y == 10);
        assert_eq!(hidden.unwrap().instance, 0, "a filtered instance is not picked");
    }
}
