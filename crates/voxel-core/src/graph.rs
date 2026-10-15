//! Editing the scene graph: moving, rotating, adding and removing instances.
//!
//! Every edit is computed as a whole new node list rather than a change in
//! place, so the app records it as one `Command::SetGraph` holding the lists
//! before and after: undo is swapping them back. Graphs are small next to the
//! voxels -- tens of nodes, rarely thousands -- so the copies are cheap.

use crate::{Instance, Scene, Transform};
use glam::{IVec3, Mat3};
use vox_format::{Dict, GroupNode, SceneNode, ShapeModel, ShapeNode, TransformNode};

/// Why a graph edit could not be made.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphError {
    NoInstance,
    /// The shape hangs straight off a group, with no transform of its own.
    NotPlaceable,
    /// No group to add a new instance to.
    NoGroup,
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoInstance => "no such instance",
            Self::NotPlaceable => "this instance has no transform node of its own",
            Self::NoGroup => "the scene has no group to add to",
        })
    }
}

impl Scene {
    /// The nodes with instance `index` placed at `world`.
    ///
    /// The node only holds the local placement; the parent chain above it
    /// stays as it is, so local = parent⁻¹ ∘ world.
    pub fn with_instance_placed(&self, index: usize, world: Transform) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let mut nodes = self.nodes.clone();
        place(&mut nodes, instance, world)?;
        Ok(nodes)
    }

    /// Moves instance `index` by a world-space offset.
    pub fn with_instance_moved(&self, index: usize, delta: IVec3) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let mut world = instance.transform;
        world.translation += delta;
        self.with_instance_placed(index, world)
    }

    /// Turns instance `index` a quarter turn about the world z axis, in
    /// place.
    pub fn with_instance_turned(&self, index: usize) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let quarter = Mat3::from_cols(glam::Vec3::Y, -glam::Vec3::X, glam::Vec3::Z);
        let mut world = instance.transform;
        world.rotation = quarter * world.rotation;
        self.with_instance_placed(index, world)
    }

    /// Shifts every instance of `model` by `delta`, given in that model's
    /// own axes. A model operation that changes the box (crop, resize,
    /// rotate) moves the pivot; this puts the voxels back where they were.
    pub fn with_model_shifted(&self, model: usize, delta: IVec3) -> Vec<SceneNode> {
        let mut nodes = self.nodes.clone();
        for instance in self.instances.iter().filter(|i| i.model_index == model) {
            let mut world = instance.transform;
            world.translation += (world.rotation * delta.as_vec3()).round().as_ivec3();
            // An instance without its own node cannot move; skip it rather
            // than fail the whole operation.
            let _ = place(&mut nodes, instance, world);
        }
        nodes
    }

    /// Adds a second instance of the same model next to instance `index`,
    /// in the same group, offset by `offset` in world space.
    pub fn with_instance_duplicated(&self, index: usize, offset: IVec3) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let node = instance.node.ok_or(GraphError::NotPlaceable)?;
        let mut nodes = self.nodes.clone();
        let group = parent_group(&nodes, node).ok_or(GraphError::NoGroup)?;
        let layer = match find(&nodes, node) {
            Some(SceneNode::Transform(t)) => t.layer,
            _ => 0,
        };

        let mut world = instance.transform;
        world.translation += offset;
        let local = world.then(&instance.parent.inverse());
        add_instance(&mut nodes, group, instance.model_index, local, layer);
        Ok(compact(nodes))
    }

    /// Adds an instance of `model` to the root group, placed at `world`
    /// (the root group's parent chain is taken as the identity, which is how
    /// every editor-written file has it).
    pub fn with_new_instance(&self, model: usize, world: Transform) -> Result<Vec<SceneNode>, GraphError> {
        let mut nodes = self.nodes.clone();
        let group = root_group(&nodes).ok_or(GraphError::NoGroup)?;
        add_instance(&mut nodes, group, model, world, 0);
        Ok(compact(nodes))
    }

    /// Removes instance `index`: its transform node from its group, and the
    /// shape below it. The model itself stays in the list, unreferenced, so
    /// model indices in the undo history keep meaning the same model.
    pub fn with_instance_removed(&self, index: usize) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let node = instance.node.ok_or(GraphError::NotPlaceable)?;
        let mut nodes = self.nodes.clone();
        let shape = match find(&nodes, node) {
            Some(SceneNode::Transform(t)) => t.child,
            _ => return Err(GraphError::NotPlaceable),
        };
        for n in &mut nodes {
            if let SceneNode::Group(g) = n {
                g.children.retain(|&c| c != node);
            }
        }
        // Only the shape when nothing else refers to it.
        let shared = nodes.iter().any(|n| matches!(n, SceneNode::Transform(t) if t.id != node && t.child == shape));
        nodes.retain(|n| n.id() != node && (shared || n.id() != shape));
        Ok(compact(nodes))
    }
}

/// Renumbers nodes 0, 1, 2... in list order, rewriting every reference.
///
/// The editor writes ids that equal each node's position in the file, and
/// readers rely on it: `dot_vox` indexes its node list by id. Removing an
/// instance would leave a gap, so every structural edit closes it.
fn compact(nodes: Vec<SceneNode>) -> Vec<SceneNode> {
    let map: std::collections::HashMap<i32, i32> =
        nodes.iter().enumerate().map(|(i, n)| (n.id(), i as i32)).collect();
    let id = |old: i32| map.get(&old).copied().unwrap_or(old);
    nodes
        .into_iter()
        .map(|n| match n {
            SceneNode::Transform(mut t) => {
                t.id = id(t.id);
                t.child = id(t.child);
                SceneNode::Transform(t)
            }
            SceneNode::Group(mut g) => {
                g.id = id(g.id);
                g.children = g.children.into_iter().map(id).collect();
                SceneNode::Group(g)
            }
            SceneNode::Shape(mut s) => {
                s.id = id(s.id);
                SceneNode::Shape(s)
            }
        })
        .collect()
}

fn find(nodes: &[SceneNode], id: i32) -> Option<&SceneNode> {
    nodes.iter().find(|n| n.id() == id)
}

fn place(nodes: &mut [SceneNode], instance: &Instance, world: Transform) -> Result<(), GraphError> {
    let node = instance.node.ok_or(GraphError::NotPlaceable)?;
    let local = world.then(&instance.parent.inverse());
    match nodes.iter_mut().find(|n| n.id() == node) {
        Some(SceneNode::Transform(t)) => {
            t.set_placement(local.translation.to_array(), local.rotation_bits());
            Ok(())
        }
        _ => Err(GraphError::NotPlaceable),
    }
}

/// The group listing `child` among its children.
fn parent_group(nodes: &[SceneNode], child: i32) -> Option<i32> {
    nodes.iter().find_map(|n| match n {
        SceneNode::Group(g) if g.children.contains(&child) => Some(g.id),
        _ => None,
    })
}

/// The group under the root transform, where the editor puts new models.
fn root_group(nodes: &[SceneNode]) -> Option<i32> {
    match nodes.first()? {
        SceneNode::Group(g) => Some(g.id),
        SceneNode::Transform(t) => match find(nodes, t.child)? {
            SceneNode::Group(g) => Some(g.id),
            _ => None,
        },
        SceneNode::Shape(_) => None,
    }
}

fn add_instance(nodes: &mut Vec<SceneNode>, group: i32, model: usize, local: Transform, layer: i32) {
    let next = nodes.iter().map(SceneNode::id).max().unwrap_or(-1) + 1;
    let mut transform = TransformNode {
        id: next,
        attributes: Dict::default(),
        child: next + 1,
        layer,
        frames: Vec::new(),
    };
    transform.set_placement(local.translation.to_array(), local.rotation_bits());
    nodes.push(SceneNode::Transform(transform));
    nodes.push(SceneNode::Shape(ShapeNode {
        id: next + 1,
        attributes: Dict::default(),
        models: vec![ShapeModel { model_id: model as i32, attributes: Dict::default() }],
    }));
    if let Some(SceneNode::Group(GroupNode { children, .. })) = nodes.iter_mut().find(|n| n.id() == group) {
        children.push(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Volume;

    /// Two models through the minimal graph, plus an outer group transform
    /// to prove parent chains are respected.
    fn scene() -> Scene {
        let models = vec![Volume::new([4, 4, 4]), Volume::new([2, 2, 2])];
        let file_models: Vec<vox_format::Model> = models.iter().map(Volume::to_model).collect();
        let mut nodes = vox_format::minimal_graph(&file_models);
        // Move the root: every instance now sits 100 voxels along x.
        if let SceneNode::Transform(t) = &mut nodes[0] {
            t.set_placement([100, 0, 0], 0b0000_0100);
        }
        let mut s = Scene { models, nodes, instances: Vec::new() };
        s.rebuild();
        s
    }

    fn apply(s: &mut Scene, nodes: Vec<SceneNode>) {
        s.nodes = nodes;
        s.rebuild();
    }

    #[test]
    fn flattening_records_the_owning_node_and_parent() {
        let s = scene();
        assert_eq!(s.instances.len(), 2);
        assert_eq!(s.instances[0].node, Some(2), "listed in file order");
        assert_eq!(s.instances[0].parent.translation, IVec3::new(100, 0, 0));
        assert_eq!(s.instances[0].transform.translation, IVec3::new(100, 0, 2));
    }

    #[test]
    fn moving_writes_the_local_placement_under_the_parent() {
        let mut s = scene();
        let nodes = s.with_instance_moved(1, IVec3::new(5, 0, 0)).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances[1].transform.translation, IVec3::new(105, 0, 1));
        match find(&s.nodes, 4) {
            Some(SceneNode::Transform(t)) => assert_eq!(t.translation(0), [5, 0, 1], "local, not world"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn turning_round_trips_through_the_rotation_byte() {
        let mut s = scene();
        for _ in 0..4 {
            let nodes = s.with_instance_turned(0).unwrap();
            apply(&mut s, nodes);
        }
        assert_eq!(s.instances[0].transform, scene().instances[0].transform);
    }

    #[test]
    fn duplicating_adds_an_instance_of_the_same_model() {
        let mut s = scene();
        let nodes = s.with_instance_duplicated(0, IVec3::new(10, 0, 0)).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances.len(), 3);
        let copy = &s.instances[2];
        assert_eq!(copy.model_index, 0);
        assert_eq!(copy.transform.translation, IVec3::new(110, 0, 2));
    }

    #[test]
    fn removing_drops_the_instance_and_its_nodes() {
        let mut s = scene();
        let before = s.nodes.len();
        let nodes = s.with_instance_removed(0).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances.len(), 1);
        assert_eq!(s.instances[0].model_index, 1);
        assert_eq!(s.nodes.len(), before - 2);
        assert_eq!(s.models.len(), 2, "the model stays");
    }

    #[test]
    fn a_new_instance_lands_in_the_root_group() {
        let mut s = scene();
        s.models.push(Volume::new([3, 3, 3]));
        let world = Transform { translation: IVec3::new(0, 0, 1), ..Default::default() };
        let nodes = s.with_new_instance(2, world).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances.len(), 3);
        assert_eq!(s.instances[2].model_index, 2);
        // The root transform still applies: placed under it, as the editor does.
        assert_eq!(s.instances[2].transform.translation, IVec3::new(100, 0, 1));
    }

    #[test]
    fn shifting_a_model_moves_each_of_its_instances_in_its_own_axes() {
        let mut s = scene();
        let nodes = s.with_instance_turned(0).unwrap();
        apply(&mut s, nodes);
        let before = s.instances[0].transform.translation;
        let nodes = s.with_model_shifted(0, IVec3::X);
        apply(&mut s, nodes);
        // Turned a quarter about z, the model's +x points along world +y.
        assert_eq!(s.instances[0].transform.translation - before, IVec3::Y);
    }

    #[test]
    fn the_edited_graph_survives_a_save() {
        let mut s = scene();
        let nodes = s.with_instance_moved(0, IVec3::new(0, 7, 0)).unwrap();
        apply(&mut s, nodes);
        let file = vox_format::VoxFile {
            nodes: s.nodes.clone(),
            models: s.models.iter().map(Volume::to_model).collect(),
            ..vox_format::VoxFile::new_single([1, 1, 1])
        };
        let back = Scene::from_file(&vox_format::VoxFile::parse(&file.to_bytes()).unwrap());
        assert_eq!(back.instances[0].transform.translation, IVec3::new(100, 7, 2));
    }

    #[test]
    fn ids_stay_contiguous_and_an_independent_reader_agrees() {
        let mut s = scene();
        let nodes = s.with_instance_removed(0).unwrap();
        apply(&mut s, nodes);
        let nodes = s.with_instance_duplicated(0, IVec3::new(9, 0, 0)).unwrap();
        apply(&mut s, nodes);
        let nodes = s.with_instance_moved(1, IVec3::new(0, 4, 0)).unwrap();
        apply(&mut s, nodes);

        for (i, n) in s.nodes.iter().enumerate() {
            assert_eq!(n.id(), i as i32, "ids equal positions");
        }

        let file = vox_format::VoxFile {
            nodes: s.nodes.clone(),
            models: s.models.iter().map(Volume::to_model).collect(),
            ..vox_format::VoxFile::new_single([1, 1, 1])
        };
        let theirs = dot_vox::load_bytes(&file.to_bytes()).expect("dot_vox reads it");
        // Walk dot_vox's graph the way it is meant to be walked, by index,
        // and collect each shape's world translation.
        let mut placed = Vec::new();
        let mut stack = vec![(0u32, [0i32; 3])];
        while let Some((index, t)) = stack.pop() {
            match &theirs.scenes[index as usize] {
                dot_vox::SceneNode::Transform { frames, child, .. } => {
                    let p = frames.first().and_then(|f| f.position()).map_or([0; 3], |p| [p.x, p.y, p.z]);
                    stack.push((*child, [t[0] + p[0], t[1] + p[1], t[2] + p[2]]));
                }
                dot_vox::SceneNode::Group { children, .. } => {
                    stack.extend(children.iter().map(|c| (*c, t)));
                }
                dot_vox::SceneNode::Shape { models, .. } => placed.push((models[0].model_id, t)),
            }
        }
        placed.sort();
        let mut ours: Vec<(u32, [i32; 3])> = s
            .instances
            .iter()
            .map(|i| (i.model_index as u32, i.transform.translation.to_array()))
            .collect();
        ours.sort();
        assert_eq!(placed, ours);
    }
}

