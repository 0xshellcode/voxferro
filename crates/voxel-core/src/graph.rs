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
    /// Not a transform node holding a group.
    NotAGroup,
    /// The root group has nothing above it to dissolve into.
    IsRoot,
}

/// One row of the scene tree, in file order, for the outline view.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    /// Nesting below the root group.
    pub depth: usize,
    /// The transform node of the row: the group's or the instance's.
    pub node: i32,
    pub kind: TreeKind,
    /// `_name`, if the node has one.
    pub name: Option<String>,
    /// Its own `_hidden`; ancestors hide it too.
    pub hidden: bool,
    pub layer: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeKind {
    Group,
    /// The index into `Scene::instances`.
    Instance(usize),
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoInstance => "no such instance",
            Self::NotPlaceable => "this instance has no transform node of its own",
            Self::NoGroup => "the scene has no group to add to",
            Self::NotAGroup => "not a group",
            Self::IsRoot => "the root group cannot be dissolved",
        })
    }
}

impl Scene {
    /// The scene as an outline: groups and instances, nested, in file order.
    /// The root transform and its group are the scene itself and get no row.
    pub fn tree(&self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        let Some(root_group) = root_group(&self.nodes) else { return rows };
        // An explicit stack, children pushed in reverse to pop in order.
        let mut stack: Vec<(i32, usize)> = match find(&self.nodes, root_group) {
            Some(SceneNode::Group(g)) => g.children.iter().rev().map(|&c| (c, 0)).collect(),
            _ => Vec::new(),
        };
        let mut guard = 0;
        while let Some((id, depth)) = stack.pop() {
            guard += 1;
            if guard > 1_000_000 {
                break;
            }
            let Some(SceneNode::Transform(t)) = find(&self.nodes, id) else { continue };
            let kind = match find(&self.nodes, t.child) {
                Some(SceneNode::Group(g)) => {
                    stack.extend(g.children.iter().rev().map(|&c| (c, depth + 1)));
                    TreeKind::Group
                }
                Some(SceneNode::Shape(_)) => match self.instances.iter().position(|i| i.node == Some(id)) {
                    Some(index) => TreeKind::Instance(index),
                    None => continue,
                },
                _ => continue,
            };
            rows.push(TreeRow {
                depth,
                node: id,
                kind,
                name: t.attributes.get("_name").map(str::to_owned),
                hidden: t.attributes.get("_hidden") == Some("1"),
                layer: t.layer,
            });
        }
        rows
    }

    /// The world placement of a transform node: where a group sits.
    pub fn node_world(&self, node: i32) -> Option<Transform> {
        let parent = parent_transforms(&self.nodes).get(&node).copied()?;
        match find(&self.nodes, node)? {
            SceneNode::Transform(t) => Some(Transform::from_parts(t.rotation_bits(0), t.translation(0)).then(&parent)),
            _ => None,
        }
    }

    /// The nodes with transform node `node` moved by a world offset. For a
    /// group this moves everything in it.
    pub fn with_node_moved(&self, node: i32, delta: IVec3) -> Result<Vec<SceneNode>, GraphError> {
        let parents = parent_transforms(&self.nodes);
        let parent = parents.get(&node).copied().ok_or(GraphError::NotPlaceable)?;
        let mut nodes = self.nodes.clone();
        let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == node) else {
            return Err(GraphError::NotPlaceable);
        };
        let local = Transform::from_parts(t.rotation_bits(0), t.translation(0));
        let mut world = local.then(&parent);
        world.translation += delta;
        let local = world.then(&parent.inverse());
        t.set_placement(local.translation.to_array(), local.rotation_bits());
        Ok(nodes)
    }

    /// The nodes with an attribute of transform node `node` set, or removed
    /// with `None`: `_name`, `_hidden`.
    pub fn with_node_attribute(&self, node: i32, key: &str, value: Option<&str>) -> Result<Vec<SceneNode>, GraphError> {
        let mut nodes = self.nodes.clone();
        let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == node) else {
            return Err(GraphError::NotPlaceable);
        };
        match value {
            Some(v) => t.attributes.set(key, v),
            None => t.attributes.remove(key),
        }
        Ok(nodes)
    }

    /// The nodes with instance `index` on another layer.
    pub fn with_instance_layer(&self, index: usize, layer: i32) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let node = instance.node.ok_or(GraphError::NotPlaceable)?;
        let mut nodes = self.nodes.clone();
        match nodes.iter_mut().find(|n| n.id() == node) {
            Some(SceneNode::Transform(t)) => t.layer = layer,
            _ => return Err(GraphError::NotPlaceable),
        }
        Ok(nodes)
    }

    /// Puts the given instances in a new group, inside the group holding the
    /// first of them. Nothing moves in the world: each instance's placement
    /// is rewritten relative to its new parent.
    pub fn with_grouped(&self, indices: &[usize]) -> Result<Vec<SceneNode>, GraphError> {
        let instances: Vec<&Instance> = indices
            .iter()
            .map(|&i| self.instances.get(i).ok_or(GraphError::NoInstance))
            .collect::<Result<_, _>>()?;
        let first = instances.first().ok_or(GraphError::NoInstance)?;
        let first_node = first.node.ok_or(GraphError::NotPlaceable)?;
        let container = parent_group(&self.nodes, first_node).ok_or(GraphError::NoGroup)?;
        // Groups add no transform, so the container's world transform is
        // what sat above the first instance's own node.
        let container_world = first.parent;

        let mut nodes = self.nodes.clone();
        let moved: Vec<i32> = instances.iter().filter_map(|i| i.node).collect();
        if moved.len() != instances.len() {
            return Err(GraphError::NotPlaceable);
        }
        for instance in &instances {
            let local = instance.transform.then(&container_world.inverse());
            if let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| Some(n.id()) == instance.node) {
                t.set_placement(local.translation.to_array(), local.rotation_bits());
            }
        }

        let next = nodes.iter().map(SceneNode::id).max().unwrap_or(-1) + 1;
        let (group_transform, group) = (next, next + 1);
        let mut slot = None;
        for n in &mut nodes {
            if let SceneNode::Group(g) = n {
                if g.id == container {
                    slot = g.children.iter().position(|c| moved.contains(c));
                }
                g.children.retain(|c| !moved.contains(c));
            }
        }
        if let Some(SceneNode::Group(g)) = nodes.iter_mut().find(|n| n.id() == container) {
            let at = slot.unwrap_or(g.children.len()).min(g.children.len());
            g.children.insert(at, group_transform);
        }
        nodes.push(SceneNode::Transform(TransformNode {
            id: group_transform,
            attributes: Dict::default(),
            child: group,
            layer: -1,
            frames: vec![Dict::default()],
        }));
        nodes.push(SceneNode::Group(GroupNode { id: group, attributes: Dict::default(), children: moved }));
        Ok(compact(nodes))
    }

    /// Dissolves the group under transform node `node` into the group above
    /// it. The group's own placement is folded into each child's, so nothing
    /// moves in the world.
    pub fn with_ungrouped(&self, node: i32) -> Result<Vec<SceneNode>, GraphError> {
        let (local, group, layer) = match find(&self.nodes, node) {
            Some(SceneNode::Transform(t)) => match find(&self.nodes, t.child) {
                Some(SceneNode::Group(g)) => {
                    (Transform::from_parts(t.rotation_bits(0), t.translation(0)), g.clone(), t.layer)
                }
                _ => return Err(GraphError::NotAGroup),
            },
            _ => return Err(GraphError::NotAGroup),
        };
        let container = parent_group(&self.nodes, node).ok_or(GraphError::IsRoot)?;

        let mut nodes = self.nodes.clone();
        for child in &group.children {
            if let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == *child) {
                let own = Transform::from_parts(t.rotation_bits(0), t.translation(0));
                let folded = own.then(&local);
                t.set_placement(folded.translation.to_array(), folded.rotation_bits());
                if t.layer < 0 {
                    t.layer = layer;
                }
            }
        }
        if let Some(SceneNode::Group(g)) = nodes.iter_mut().find(|n| n.id() == container) {
            if let Some(at) = g.children.iter().position(|&c| c == node) {
                g.children.splice(at..=at, group.children.iter().copied());
            }
        }
        nodes.retain(|n| n.id() != node && n.id() != group.id);
        Ok(compact(nodes))
    }

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

/// Puts the nodes in depth-first pre-order from the root and renumbers them
/// 0, 1, 2... in that order, rewriting every reference.
///
/// That is exactly how the editor writes a graph: each transform followed by
/// its child, each group's children in increasing order. Readers may rely on
/// it -- `dot_vox` indexes its node list by id -- so an edited graph keeps
/// the shape of one the editor wrote. Unreachable nodes, if any, follow in
/// their old order.
fn compact(nodes: Vec<SceneNode>) -> Vec<SceneNode> {
    let mut order: Vec<usize> = Vec::with_capacity(nodes.len());
    let mut seen = vec![false; nodes.len()];
    let index_of: std::collections::HashMap<i32, usize> =
        nodes.iter().enumerate().map(|(i, n)| (n.id(), i)).collect();
    let mut stack: Vec<i32> = nodes.first().map(SceneNode::id).into_iter().collect();
    while let Some(id) = stack.pop() {
        let Some(&i) = index_of.get(&id) else { continue };
        if std::mem::replace(&mut seen[i], true) {
            continue; // shared or cyclic: visit once
        }
        order.push(i);
        match &nodes[i] {
            SceneNode::Transform(t) => stack.push(t.child),
            SceneNode::Group(g) => stack.extend(g.children.iter().rev()),
            SceneNode::Shape(_) => {}
        }
    }
    order.extend((0..nodes.len()).filter(|&i| !seen[i]));

    let map: std::collections::HashMap<i32, i32> =
        order.iter().enumerate().map(|(new, &old)| (nodes[old].id(), new as i32)).collect();
    let id = |old: i32| map.get(&old).copied().unwrap_or(old);
    let mut slots: Vec<Option<SceneNode>> = nodes.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|i| slots[i].take())
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

/// The world transform above every reachable transform node: what its own
/// placement is relative to.
fn parent_transforms(nodes: &[SceneNode]) -> std::collections::HashMap<i32, Transform> {
    let mut out = std::collections::HashMap::new();
    let Some(root) = nodes.first().map(SceneNode::id) else { return out };
    let mut stack = vec![(root, Transform::default())];
    while let Some((id, above)) = stack.pop() {
        if out.len() > nodes.len() {
            break; // a cycle in a malformed file
        }
        match find(nodes, id) {
            Some(SceneNode::Transform(t)) => {
                if out.insert(t.id, above).is_some() {
                    continue;
                }
                let local = Transform::from_parts(t.rotation_bits(0), t.translation(0));
                stack.push((t.child, local.then(&above)));
            }
            Some(SceneNode::Group(g)) => stack.extend(g.children.iter().map(|&c| (c, above))),
            _ => {}
        }
    }
    out
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

        assert_depth_first(&s.nodes);

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

    #[test]
    fn grouping_keeps_everything_in_place_and_ungrouping_undoes_it() {
        let mut s = scene();
        let before: Vec<_> = s.instances.iter().map(|i| i.transform).collect();
        let nodes = s.with_grouped(&[0, 1]).unwrap();
        apply(&mut s, nodes);
        let after: Vec<_> = s.instances.iter().map(|i| i.transform).collect();
        assert_eq!(before, after, "grouping moves nothing");
        let tree = s.tree();
        assert_eq!(tree[0].kind, TreeKind::Group);
        assert_eq!((tree[1].depth, tree[2].depth), (1, 1), "both inside it");

        // Move the group: both instances follow.
        let group = tree[0].node;
        let nodes = s.with_node_moved(group, IVec3::new(0, 0, 5)).unwrap();
        apply(&mut s, nodes);
        assert!(s.instances.iter().zip(&before).all(|(i, b)| i.transform.translation == b.translation + IVec3::Z * 5));

        // Ungrouping folds the group's offset into the children.
        let group = s.tree()[0].node;
        let nodes = s.with_ungrouped(group).unwrap();
        apply(&mut s, nodes);
        assert!(s.tree().iter().all(|r| r.depth == 0 && matches!(r.kind, TreeKind::Instance(_))));
        assert!(s.instances.iter().zip(&before).all(|(i, b)| i.transform.translation == b.translation + IVec3::Z * 5));
        assert_depth_first(&s.nodes);
    }

    /// Ids equal positions, and the list is in depth-first pre-order: each
    /// transform is followed by its child, each group's children come in
    /// increasing order after it -- the order the editor writes.
    fn assert_depth_first(nodes: &[SceneNode]) {
        for (i, n) in nodes.iter().enumerate() {
            assert_eq!(n.id(), i as i32, "ids equal positions");
            match n {
                SceneNode::Transform(t) => assert_eq!(t.child, t.id + 1, "child right after"),
                SceneNode::Group(g) => {
                    assert!(g.children.windows(2).all(|w| w[0] < w[1]), "children in order: {:?}", g.children);
                    assert!(g.children.first().is_none_or(|&c| c == g.id + 1));
                }
                SceneNode::Shape(_) => {}
            }
        }
    }

    #[test]
    fn a_group_made_after_a_sibling_is_written_in_editor_order() {
        // The case the reference editor misread: group the first two of three.
        let mut s = scene();
        s.models.push(Volume::new([3, 3, 3]));
        let nodes = s.with_new_instance(2, Transform::default()).unwrap();
        apply(&mut s, nodes);
        let before: Vec<_> = s.instances.iter().map(|i| (i.model_index, i.transform)).collect();
        let nodes = s.with_grouped(&[0, 1]).unwrap();
        apply(&mut s, nodes);
        assert_depth_first(&s.nodes);
        let mut after: Vec<_> = s.instances.iter().map(|i| (i.model_index, i.transform)).collect();
        after.sort_by_key(|a| a.0);
        assert_eq!(before, after, "same placements");
    }

    #[test]
    fn a_hidden_group_hides_what_it_holds() {
        let mut s = scene();
        let nodes = s.with_grouped(&[1]).unwrap();
        apply(&mut s, nodes);
        let group = s.tree().iter().find(|r| r.kind == TreeKind::Group).unwrap().node;
        let nodes = s.with_node_attribute(group, "_hidden", Some("1")).unwrap();
        apply(&mut s, nodes);
        let hidden: Vec<bool> = s.instances.iter().map(|i| i.hidden).collect();
        assert_eq!(hidden.iter().filter(|h| **h).count(), 1);
    }

    #[test]
    fn only_groups_below_the_root_can_be_dissolved() {
        let s = scene();
        // Node 0 is the root transform: its group has no container.
        assert_eq!(s.with_ungrouped(0), Err(GraphError::IsRoot));
        let instance = s.instances[0].node.unwrap();
        assert_eq!(s.with_ungrouped(instance), Err(GraphError::NotAGroup));
    }

    #[test]
    fn layers_and_names_are_written_to_the_node() {
        let mut s = scene();
        let nodes = s.with_instance_layer(1, 3).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances[1].layer, 3);
        let node = s.instances[1].node.unwrap();
        let nodes = s.with_node_attribute(node, "_name", Some("car")).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.tree()[1].name.as_deref(), Some("car"));
    }
}

