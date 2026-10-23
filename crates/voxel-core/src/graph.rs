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
    /// A group cannot go inside itself or anything it holds.
    IntoItself,
    /// No key on this frame, or it is the only one.
    NoKey,
    /// Moving a key onto a frame that already has one.
    FrameTaken,
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
            Self::IntoItself => "a group cannot go inside itself",
            Self::NoKey => "no key on that frame (or it is the only one)",
            Self::FrameTaken => "that frame already has a key",
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
        let parent = parent_transforms(&self.nodes, self.frame).get(&node).copied()?;
        match find(&self.nodes, node)? {
            SceneNode::Transform(t) => Some(crate::anim::local_at(t, self.frame).then(&parent)),
            _ => None,
        }
    }

    /// The groups holding instance `index`, outermost first, by their
    /// transform nodes. Empty when it sits directly in the scene.
    pub fn groups_above(&self, index: usize) -> Vec<i32> {
        let rows = self.tree();
        let Some(at) = rows.iter().position(|r| r.kind == TreeKind::Instance(index)) else {
            return Vec::new();
        };
        // Walking back up the outline, each shallower group row is the next
        // container out.
        let mut depth = rows[at].depth;
        let mut out = Vec::new();
        for row in rows[..at].iter().rev() {
            if row.kind == TreeKind::Group && row.depth < depth {
                out.push(row.node);
                depth = row.depth;
            }
        }
        out.reverse();
        out
    }

    /// Transform nodes in the subtree under `node`, itself included.
    fn subtree(&self, node: i32) -> Vec<i32> {
        let mut out = Vec::new();
        let mut stack = vec![node];
        while let Some(id) = stack.pop() {
            if out.len() > self.nodes.len() {
                break; // a cycle in a malformed file
            }
            match find(&self.nodes, id) {
                Some(SceneNode::Transform(t)) => {
                    out.push(t.id);
                    stack.push(t.child);
                }
                Some(SceneNode::Group(g)) => stack.extend(g.children.iter().copied()),
                _ => {}
            }
        }
        out
    }

    /// The instances under transform node `node`: what a group holds, or
    /// the instance itself.
    pub fn instances_under(&self, node: i32) -> Vec<usize> {
        let ids = self.subtree(node);
        (0..self.instances.len())
            .filter(|&i| self.instances[i].node.is_some_and(|n| ids.contains(&n)))
            .collect()
    }

    /// The nodes with transform node `node` given a quarter turn about a
    /// world axis (0 x, 1 y, 2 z), around `centre`. For a group that is the
    /// middle of what it holds, so it turns in place instead of swinging
    /// around an origin that may be far away.
    pub fn with_node_turned(&self, node: i32, centre: IVec3, axis: usize) -> Result<Vec<SceneNode>, GraphError> {
        self.with_node_turned_times(node, centre, axis, 1)
    }

    /// Like [`Scene::with_node_turned`], by `times` quarter turns; 3 is a
    /// clockwise quarter.
    pub fn with_node_turned_times(
        &self,
        node: i32,
        centre: IVec3,
        axis: usize,
        times: u32,
    ) -> Result<Vec<SceneNode>, GraphError> {
        let parents = parent_transforms(&self.nodes, self.frame);
        let parent = parents.get(&node).copied().ok_or(GraphError::NotPlaceable)?;
        let mut nodes = self.nodes.clone();
        let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == node) else {
            return Err(GraphError::NotPlaceable);
        };
        let mut quarter = Mat3::IDENTITY;
        for _ in 0..times % 4 {
            quarter = quarter_turn(axis) * quarter;
        }
        let local = crate::anim::local_at(t, self.frame);
        let world = local.then(&parent);
        let turned = Transform {
            rotation: quarter * world.rotation,
            translation: (quarter * (world.translation - centre).as_vec3()).round().as_ivec3() + centre,
        };
        let local = turned.then(&parent.inverse());
        crate::anim::set_local_at(t, self.frame, local);
        Ok(nodes)
    }

    /// Moves transform node `node` (an instance or a group) into the group
    /// under transform node `target`, or into the root group when `target`
    /// is `None`. Nothing moves in the world: the placement is rewritten
    /// relative to the new parent.
    pub fn with_node_moved_into(&self, node: i32, target: Option<i32>) -> Result<Vec<SceneNode>, GraphError> {
        let parents = parent_transforms(&self.nodes, self.frame);
        let (group, group_world) = match target {
            Some(target) => {
                if self.subtree(node).contains(&target) {
                    return Err(GraphError::IntoItself);
                }
                let world = self.node_world(target).ok_or(GraphError::NotAGroup)?;
                match find(&self.nodes, target) {
                    Some(SceneNode::Transform(t)) => match find(&self.nodes, t.child) {
                        Some(SceneNode::Group(g)) => (g.id, world),
                        _ => return Err(GraphError::NotAGroup),
                    },
                    _ => return Err(GraphError::NotAGroup),
                }
            }
            None => {
                let root = self.nodes.first().map(SceneNode::id).ok_or(GraphError::NoGroup)?;
                (root_group(&self.nodes).ok_or(GraphError::NoGroup)?, self.node_world(root).unwrap_or_default())
            }
        };
        let parent = parents.get(&node).copied().ok_or(GraphError::NotPlaceable)?;
        let mut nodes = self.nodes.clone();
        let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == node) else {
            return Err(GraphError::NotPlaceable);
        };
        let world = crate::anim::local_at(t, self.frame).then(&parent);
        let local = world.then(&group_world.inverse());
        crate::anim::set_local_at(t, self.frame, local);
        for n in &mut nodes {
            if let SceneNode::Group(g) = n {
                g.children.retain(|&c| c != node);
                if g.id == group {
                    g.children.push(node);
                }
            }
        }
        Ok(compact(nodes))
    }

    /// The nodes with instance `index` showing `model` from `frame` on: a
    /// shape's frame of animation. A shape that was not animated keeps its
    /// model as frame 0.
    pub fn with_shape_frame(&self, index: usize, model: usize, frame: u32) -> Result<Vec<SceneNode>, GraphError> {
        let node = self.instances.get(index).ok_or(GraphError::NoInstance)?.node.ok_or(GraphError::NotPlaceable)?;
        let shape = match find(&self.nodes, node) {
            Some(SceneNode::Transform(t)) => t.child,
            _ => return Err(GraphError::NotPlaceable),
        };
        let mut nodes = self.nodes.clone();
        let Some(SceneNode::Shape(s)) = nodes.iter_mut().find(|n| n.id() == shape) else {
            return Err(GraphError::NotPlaceable);
        };
        for m in &mut s.models {
            if m.attributes.get("_f").is_none() {
                m.attributes.set("_f", "0");
            }
        }
        let keyed = |m: &ShapeModel| m.attributes.get("_f").and_then(|v| v.parse::<u32>().ok()) == Some(frame);
        match s.models.iter_mut().find(|m| keyed(m)) {
            Some(m) => m.model_id = model as i32,
            None => {
                s.models.push(ShapeModel {
                    model_id: model as i32,
                    attributes: Dict(vec![("_f".into(), frame.to_string())]),
                });
                s.models.sort_by_key(|m| m.attributes.get("_f").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0));
            }
        }
        Ok(nodes)
    }

    /// The nodes with the key of transform node `node` moved from frame
    /// `from` to frame `to` -- or, with `shape`, the model its shape shows
    /// from `from`.
    pub fn with_key_moved(&self, node: i32, from: u32, to: u32, shape: bool) -> Result<Vec<SceneNode>, GraphError> {
        let mut nodes = self.nodes.clone();
        let child = match find(&nodes, node) {
            Some(SceneNode::Transform(t)) => t.child,
            _ => return Err(GraphError::NotPlaceable),
        };
        let moved = if shape {
            match nodes.iter_mut().find(|n| n.id() == child) {
                Some(SceneNode::Shape(s)) => crate::anim::move_shape_frame(s, from, to),
                _ => false,
            }
        } else {
            match nodes.iter_mut().find(|n| n.id() == node) {
                Some(SceneNode::Transform(t)) => crate::anim::move_key(t, from, to),
                _ => false,
            }
        };
        if moved {
            return Ok(nodes);
        }
        // Say why: the usual refusal is a frame already keyed.
        let taken = match find(&self.nodes, node) {
            Some(SceneNode::Transform(t)) if !shape => crate::anim::keyframes(t).iter().any(|k| k.0 == to),
            Some(SceneNode::Transform(t)) => match find(&self.nodes, t.child) {
                Some(SceneNode::Shape(s)) => s.models.iter().any(|m| m.attributes.get("_f").and_then(|v| v.parse().ok()) == Some(to)),
                _ => false,
            },
            _ => false,
        };
        Err(if taken { GraphError::FrameTaken } else { GraphError::NoKey })
    }

    /// The nodes without the key transform node `node` has on the current
    /// frame. Its only key cannot go.
    /// The nodes with the keys of transform node `node` on `frames` removed
    /// -- all but one, if they were all of them.
    pub fn with_keys_removed(&self, node: i32, frames: &[u32]) -> Result<Vec<SceneNode>, GraphError> {
        let mut nodes = self.nodes.clone();
        let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == node) else {
            return Err(GraphError::NotPlaceable);
        };
        let mut removed = false;
        for &f in frames {
            removed |= crate::anim::remove_key(t, f);
        }
        if !removed {
            return Err(GraphError::NoKey);
        }
        Ok(nodes)
    }

    pub fn with_key_removed(&self, node: i32) -> Result<Vec<SceneNode>, GraphError> {
        let mut nodes = self.nodes.clone();
        let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == node) else {
            return Err(GraphError::NotPlaceable);
        };
        if !crate::anim::remove_key(t, self.frame) {
            return Err(GraphError::NoKey);
        }
        Ok(nodes)
    }

    /// The nodes with several transform nodes given new world placements
    /// at once -- moving, turning or aligning a multiple selection. Each
    /// is rewritten under its own parent, which none of the others may be
    /// inside (see [`Scene::outermost`]).
    pub fn with_worlds(&self, changes: &[(i32, Transform)]) -> Result<Vec<SceneNode>, GraphError> {
        let parents = parent_transforms(&self.nodes, self.frame);
        let mut nodes = self.nodes.clone();
        for &(node, world) in changes {
            let parent = parents.get(&node).copied().ok_or(GraphError::NotPlaceable)?;
            let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == node) else {
                return Err(GraphError::NotPlaceable);
            };
            crate::anim::set_local_at(t, self.frame, world.then(&parent.inverse()));
        }
        Ok(nodes)
    }

    /// `nodes` without any that sit inside another of them, so a group and
    /// a model in it, both selected, move once.
    pub fn outermost(&self, nodes: &[i32]) -> Vec<i32> {
        let mut out: Vec<i32> = Vec::new();
        for &n in nodes {
            let inside_other = nodes.iter().any(|&o| o != n && self.subtree(o).contains(&n));
            if !inside_other && !out.contains(&n) {
                out.push(n);
            }
        }
        out
    }

    /// The frames keyed on transform node `node` and on its shape, if it
    /// shows one: every `_f` there is, sorted, without repeats.
    pub fn keys_of(&self, node: i32) -> Vec<u32> {
        let frame = |d: &Dict| d.get("_f").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let mut out = Vec::new();
        if let Some(SceneNode::Transform(t)) = find(&self.nodes, node) {
            out.extend(t.frames.iter().map(frame));
            if let Some(SceneNode::Shape(s)) = find(&self.nodes, t.child) {
                out.extend(s.models.iter().map(|m| frame(&m.attributes)));
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The nodes with `_loop` set on transform node `node`, and on its
    /// shape: its keys and its models repeat.
    pub fn with_loop(&self, node: i32, on: bool) -> Result<Vec<SceneNode>, GraphError> {
        let mut nodes = self.nodes.clone();
        let child = match nodes.iter_mut().find(|n| n.id() == node) {
            Some(SceneNode::Transform(t)) => {
                if on { t.attributes.set("_loop", "1") } else { t.attributes.remove("_loop") }
                t.child
            }
            _ => return Err(GraphError::NotPlaceable),
        };
        if let Some(SceneNode::Shape(s)) = nodes.iter_mut().find(|n| n.id() == child) {
            if on { s.attributes.set("_loop", "1") } else { s.attributes.remove("_loop") }
        }
        Ok(nodes)
    }

    /// The nodes with the keys of transform node `node` (and of its shape)
    /// played backwards: each key at frame f moves to first + last - f.
    pub fn with_keys_reversed(&self, node: i32) -> Result<Vec<SceneNode>, GraphError> {
        let keys = self.keys_of(node);
        let (Some(&first), Some(&last)) = (keys.first(), keys.last()) else { return Err(GraphError::NotPlaceable) };
        let flip = |d: &mut Dict| {
            let f = d.get("_f").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
            d.set("_f", (first + last - f).to_string());
        };
        let mut nodes = self.nodes.clone();
        let child = match nodes.iter_mut().find(|n| n.id() == node) {
            Some(SceneNode::Transform(t)) => {
                t.frames.iter_mut().for_each(flip);
                t.frames.sort_by_key(|d| d.get("_f").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0));
                t.child
            }
            _ => return Err(GraphError::NotPlaceable),
        };
        if let Some(SceneNode::Shape(s)) = nodes.iter_mut().find(|n| n.id() == child)
            && s.models.len() > 1
        {
            s.models.iter_mut().for_each(|m| flip(&mut m.attributes));
            s.models.sort_by_key(|m| m.attributes.get("_f").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0));
        }
        Ok(nodes)
    }

    /// The nodes with transform node `node` moved by a world offset. For a
    /// group this moves everything in it.
    pub fn with_node_moved(&self, node: i32, delta: IVec3) -> Result<Vec<SceneNode>, GraphError> {
        let parents = parent_transforms(&self.nodes, self.frame);
        let parent = parents.get(&node).copied().ok_or(GraphError::NotPlaceable)?;
        let mut nodes = self.nodes.clone();
        let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == node) else {
            return Err(GraphError::NotPlaceable);
        };
        let local = crate::anim::local_at(t, self.frame);
        let mut world = local.then(&parent);
        world.translation += delta;
        let local = world.then(&parent.inverse());
        crate::anim::set_local_at(t, self.frame, local);
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

    /// The nodes with an attribute set, or removed with `None`, on several
    /// transform nodes at once: hiding or showing a selection in one step.
    pub fn with_nodes_attribute(&self, nodes: &[i32], key: &str, value: Option<&str>) -> Result<Vec<SceneNode>, GraphError> {
        let mut out = self.nodes.clone();
        for node in &mut out {
            if let SceneNode::Transform(t) = node
                && nodes.contains(&t.id)
            {
                match value {
                    Some(v) => t.attributes.set(key, v),
                    None => t.attributes.remove(key),
                }
            }
        }
        Ok(out)
    }

    /// The nodes with `hide` hidden and `show` shown, in one step.
    pub fn with_visibility(&self, hide: &[i32], show: &[i32]) -> Result<Vec<SceneNode>, GraphError> {
        let mut out = self.nodes.clone();
        for node in &mut out {
            if let SceneNode::Transform(t) = node {
                if show.contains(&t.id) {
                    t.attributes.remove("_hidden");
                } else if hide.contains(&t.id) {
                    t.attributes.set("_hidden", "1");
                }
            }
        }
        Ok(out)
    }

    /// The transform nodes directly inside a group, in order, by the
    /// group's transform node; `None` is the scene itself (the root group).
    pub fn children_of(&self, group: Option<i32>) -> Vec<i32> {
        let id = match group {
            None => root_group(&self.nodes),
            Some(node) => match find(&self.nodes, node) {
                Some(SceneNode::Transform(t)) => Some(t.child),
                _ => None,
            },
        };
        match id.and_then(|id| find(&self.nodes, id)) {
            Some(SceneNode::Group(g)) => g.children.clone(),
            _ => Vec::new(),
        }
    }

    /// Transform node `node` and every transform node under it.
    pub fn nodes_under(&self, node: i32) -> Vec<i32> {
        self.subtree(node)
    }

    /// The transform nodes from the outermost group down to `node`, itself
    /// included: the groups it sits in. Empty if it is not in the tree.
    pub fn path_to(&self, node: i32) -> Vec<i32> {
        let mut out = vec![node];
        let root = root_group(&self.nodes);
        let mut at = node;
        while let Some(group) = parent_group(&self.nodes, at) {
            if Some(group) == root || out.len() > self.nodes.len() {
                break;
            }
            // The transform node holding that group.
            let Some(holder) = self.nodes.iter().find_map(|n| match n {
                SceneNode::Transform(t) if t.child == group => Some(t.id),
                _ => None,
            }) else {
                break;
            };
            out.push(holder);
            at = holder;
        }
        out.reverse();
        out
    }

    /// The nodes with the children of a group (`None`: the scene) sorted by
    /// name, as the reference editor's `sort` does: only the `picked` ones when two
    /// or more are picked, each into one of their own places, the others
    /// staying where they are; all of them otherwise. Names compare byte
    /// by byte, so capitals come first and unnamed nodes before any name.
    /// Already in order, they are reversed instead.
    pub fn with_children_sorted(&self, group: Option<i32>, picked: &[i32]) -> Result<Vec<SceneNode>, GraphError> {
        let children = self.children_of(group);
        let chosen: Vec<i32> = if picked.iter().filter(|n| children.contains(n)).count() >= 2 {
            children.iter().copied().filter(|n| picked.contains(n)).collect()
        } else {
            children.clone()
        };
        let name = |id: i32| match find(&self.nodes, id) {
            Some(SceneNode::Transform(t)) => t.attributes.get("_name").unwrap_or("").as_bytes().to_vec(),
            _ => Vec::new(),
        };
        let mut sorted = chosen.clone();
        sorted.sort_by_key(|&id| name(id));
        if sorted == chosen {
            sorted.reverse();
        }
        let mut next = sorted.into_iter();
        let order: Vec<i32> = children
            .iter()
            .map(|&c| if chosen.contains(&c) { next.next().unwrap_or(c) } else { c })
            .collect();

        let group_id = match group {
            None => root_group(&self.nodes),
            Some(node) => match find(&self.nodes, node) {
                Some(SceneNode::Transform(t)) => Some(t.child),
                _ => None,
            },
        }
        .ok_or(GraphError::NotAGroup)?;
        let mut nodes = self.nodes.clone();
        match nodes.iter_mut().find(|n| n.id() == group_id) {
            Some(SceneNode::Group(g)) => g.children = order,
            _ => return Err(GraphError::NotAGroup),
        }
        Ok(compact(nodes))
    }

    /// The nodes with instances packed into one animated object, as
    /// the reference editor's `opack` (Pack Models to Animation) does.
    ///
    /// `indices` are taken in outline order. The new object takes the first
    /// one's place in its group, its name, layer and placement; its shape
    /// shows each instance's model on a frame of its own -- 0, 1, 2... when
    /// the first instance has no keys, else each one's first key's frame
    /// (instances without keys are dropped then, as are later ones on a
    /// frame already taken). With `xform`, each frame is also keyed at that
    /// instance's placement, unless they are all placed alike; with
    /// `looped`, the shape loops (`_loop`). The packed instances go.
    pub fn with_packed(&self, indices: &[usize], xform: bool, looped: bool) -> Result<Vec<SceneNode>, GraphError> {
        let mut picked: Vec<&Instance> = Vec::new();
        for &i in indices {
            let instance = self.instances.get(i).ok_or(GraphError::NoInstance)?;
            instance.node.ok_or(GraphError::NotPlaceable)?;
            picked.push(instance);
        }
        let first = *picked.first().ok_or(GraphError::NoInstance)?;
        let transform_of = |inst: &Instance| match find(&self.nodes, inst.node.unwrap_or(-1)) {
            Some(SceneNode::Transform(t)) => Some(t),
            _ => None,
        };
        let head = transform_of(first).ok_or(GraphError::NotPlaceable)?;
        let group = parent_group(&self.nodes, head.id).ok_or(GraphError::NoGroup)?;

        // Each instance's frame.
        let mut framed: Vec<(i64, &Instance)> = if !crate::anim::is_keyed(head) {
            picked.iter().enumerate().map(|(k, &inst)| (k as i64, inst)).collect()
        } else {
            picked
                .iter()
                .map(|&inst| {
                    let f = transform_of(inst)
                        .filter(|t| crate::anim::is_keyed(t))
                        .and_then(|t| crate::anim::keyframes(t).first().map(|k| k.0 as i64))
                        .unwrap_or(-1);
                    (f, inst)
                })
                .collect()
        };
        framed.sort_by_key(|(f, _)| *f);
        framed.dedup_by_key(|(f, _)| *f);
        framed.retain(|(f, _)| *f >= 0);

        // Placements under the first instance's parent.
        let local = |inst: &Instance| inst.transform.then(&first.parent.inverse());
        let next = self.nodes.iter().map(SceneNode::id).max().unwrap_or(-1) + 1;
        let mut transform = TransformNode { id: next, attributes: Dict::default(), child: next + 1, layer: head.layer, frames: Vec::new() };
        if let Some(name) = head.attributes.get("_name") {
            transform.attributes.set("_name", name);
        }
        let differ = framed.iter().any(|(_, inst)| local(inst) != local(first));
        if xform && differ {
            for (f, inst) in &framed {
                crate::anim::set_local_at(&mut transform, *f as u32, local(inst));
            }
        } else {
            let l = local(first);
            transform.set_placement(l.translation.to_array(), l.rotation_bits());
        }
        let mut shape = ShapeNode { id: next + 1, attributes: Dict::default(), models: Vec::new() };
        for (f, inst) in &framed {
            shape.models.push(ShapeModel {
                model_id: inst.model_index as i32,
                attributes: Dict(vec![("_f".into(), f.to_string())]),
            });
        }
        if looped {
            shape.attributes.set("_loop", "1");
        }

        let mut nodes = self.nodes.clone();
        let gone: Vec<i32> = picked.iter().filter_map(|inst| transform_of(inst)).flat_map(|t| [t.id, t.child]).collect();
        if let Some(SceneNode::Group(g)) = nodes.iter_mut().find(|n| n.id() == group) {
            let at = g.children.iter().position(|&c| c == head.id).unwrap_or(g.children.len());
            g.children.insert(at, next);
        }
        for node in &mut nodes {
            if let SceneNode::Group(g) = node {
                g.children.retain(|c| !gone.contains(c));
            }
        }
        nodes.retain(|n| !gone.contains(&n.id()));
        nodes.push(SceneNode::Transform(transform));
        nodes.push(SceneNode::Shape(shape));
        Ok(compact(nodes))
    }

    /// The nodes with an animated instance unpacked, as the reference editor's
    /// `ounpack`: each model of its shape becomes an instance of its own, in
    /// its place, placed as the key at that model's frame (stepping between
    /// keys, not interpolating) and named as the original.
    pub fn with_unpacked(&self, index: usize) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let node = instance.node.ok_or(GraphError::NotPlaceable)?;
        let Some(SceneNode::Transform(t)) = find(&self.nodes, node) else { return Err(GraphError::NotPlaceable) };
        let Some(SceneNode::Shape(s)) = find(&self.nodes, t.child) else { return Err(GraphError::NotPlaceable) };
        if s.models.len() < 2 {
            return Err(GraphError::NoKey);
        }
        let group = parent_group(&self.nodes, t.id).ok_or(GraphError::NoGroup)?;
        let frame_of = |m: &ShapeModel| m.attributes.get("_f").and_then(|v| v.parse::<u32>().ok()).unwrap_or(0);
        let mut models = s.models.clone();
        models.sort_by_key(frame_of);

        let mut nodes = self.nodes.clone();
        let mut next = nodes.iter().map(SceneNode::id).max().unwrap_or(-1) + 1;
        let mut added = Vec::new();
        for m in models {
            let local = crate::anim::key_at(t, frame_of(&m));
            let mut transform = TransformNode { id: next, attributes: Dict::default(), child: next + 1, layer: t.layer, frames: Vec::new() };
            if let Some(name) = t.attributes.get("_name") {
                transform.attributes.set("_name", name);
            }
            transform.set_placement(local.translation.to_array(), local.rotation_bits());
            nodes.push(SceneNode::Transform(transform));
            nodes.push(SceneNode::Shape(ShapeNode { id: next + 1, attributes: Dict::default(), models: vec![m] }));
            added.push(next);
            next += 2;
        }
        if let Some(SceneNode::Group(g)) = nodes.iter_mut().find(|n| n.id() == group) {
            let at = g.children.iter().position(|&c| c == t.id).unwrap_or(g.children.len());
            g.children.splice(at..=at.min(g.children.len().saturating_sub(1)), added);
        }
        let gone = [t.id, t.child];
        nodes.retain(|n| !gone.contains(&n.id()));
        Ok(compact(nodes))
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
                crate::anim::set_local_at(t, self.frame, local);
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
                    (crate::anim::local_at(t, self.frame), g.clone(), t.layer)
                }
                _ => return Err(GraphError::NotAGroup),
            },
            _ => return Err(GraphError::NotAGroup),
        };
        let container = parent_group(&self.nodes, node).ok_or(GraphError::IsRoot)?;

        let mut nodes = self.nodes.clone();
        for child in &group.children {
            if let Some(SceneNode::Transform(t)) = nodes.iter_mut().find(|n| n.id() == *child) {
                let own = crate::anim::local_at(t, self.frame);
                let folded = own.then(&local);
                crate::anim::set_local_at(t, self.frame, folded);
                if t.layer < 0 {
                    t.layer = layer;
                }
            }
        }
        if let Some(SceneNode::Group(g)) = nodes.iter_mut().find(|n| n.id() == container)
            && let Some(at) = g.children.iter().position(|&c| c == node) {
                g.children.splice(at..=at, group.children.iter().copied());
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
        place(&mut nodes, instance, world, self.frame)?;
        Ok(nodes)
    }

    /// Moves instance `index` by a world-space offset.
    pub fn with_instance_moved(&self, index: usize, delta: IVec3) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let mut world = instance.transform;
        world.translation += delta;
        self.with_instance_placed(index, world)
    }

    /// Turns instance `index` a quarter turn about a world axis (0 x, 1 y,
    /// 2 z), in place.
    pub fn with_instance_turned(&self, index: usize, axis: usize) -> Result<Vec<SceneNode>, GraphError> {
        let instance = self.instances.get(index).ok_or(GraphError::NoInstance)?;
        let quarter = quarter_turn(axis);
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
            let _ = place(&mut nodes, instance, world, self.frame);
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

/// A quarter turn about axis 0 (x), 1 (y) or 2 (z), counter-clockwise
/// looking down the axis from its positive end -- the same sense as
/// `ops::rotate` turns a model's contents.
pub fn quarter_turn(axis: usize) -> Mat3 {
    use glam::Vec3;
    match axis {
        // y -> z, z -> -y
        0 => Mat3::from_cols(Vec3::X, Vec3::Z, -Vec3::Y),
        // z -> x, x -> -z
        1 => Mat3::from_cols(-Vec3::Z, Vec3::Y, Vec3::X),
        // x -> y, y -> -x
        _ => Mat3::from_cols(Vec3::Y, -Vec3::X, Vec3::Z),
    }
}

/// The world transform above every reachable transform node: what its own
/// placement is relative to.
fn parent_transforms(nodes: &[SceneNode], frame: u32) -> std::collections::HashMap<i32, Transform> {
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
                let local = crate::anim::local_at(t, frame);
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

fn place(nodes: &mut [SceneNode], instance: &Instance, world: Transform, frame: u32) -> Result<(), GraphError> {
    let node = instance.node.ok_or(GraphError::NotPlaceable)?;
    let local = world.then(&instance.parent.inverse());
    match nodes.iter_mut().find(|n| n.id() == node) {
        Some(SceneNode::Transform(t)) => {
            crate::anim::set_local_at(t, frame, local);
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
        let mut s = Scene { models, nodes, instances: Vec::new(), frame: 0, held: None };
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
    fn several_nodes_are_placed_at_once_and_nested_ones_dropped() {
        let mut s = scene();
        let (a, b) = (s.instances[0].node.unwrap(), s.instances[1].node.unwrap());
        // The root transform (0) holds both: alone it is the outermost.
        assert_eq!(s.outermost(&[a, 0, b]), vec![0]);
        assert_eq!(s.outermost(&[a, b, a]), vec![a, b]);
        let moved: Vec<_> = [a, b]
            .iter()
            .map(|&n| {
                let w = s.node_world(n).unwrap();
                (n, Transform { translation: w.translation + IVec3::new(0, 7, 0), ..w })
            })
            .collect();
        let nodes = s.with_worlds(&moved).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances[0].transform.translation, IVec3::new(100, 7, 2));
        assert_eq!(s.instances[1].transform.translation.y, 7, "both, under the root's offset");
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
            let nodes = s.with_instance_turned(0, 2).unwrap();
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
        let nodes = s.with_instance_turned(0, 2).unwrap();
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

    fn world_of(s: &Scene) -> Vec<(usize, Transform)> {
        let mut v: Vec<_> = s.instances.iter().map(|i| (i.model_index, i.transform)).collect();
        v.sort_by_key(|a| a.0);
        v
    }

    #[test]
    fn moving_into_and_out_of_a_group_moves_nothing() {
        let mut s = scene();
        let nodes = s.with_grouped(&[0]).unwrap();
        apply(&mut s, nodes);
        let group = s.tree().iter().find(|r| r.kind == TreeKind::Group).unwrap().node;
        // Give the group a placement of its own, so the rewrite matters.
        let nodes = s.with_node_moved(group, IVec3::new(3, 4, 5)).unwrap();
        apply(&mut s, nodes);
        let before = world_of(&s);

        let other = s.instances.iter().position(|i| i.model_index == 1).unwrap();
        let node = s.instances[other].node.unwrap();
        let group = s.tree().iter().find(|r| r.kind == TreeKind::Group).unwrap().node;
        let nodes = s.with_node_moved_into(node, Some(group)).unwrap();
        apply(&mut s, nodes);
        assert_eq!(world_of(&s), before, "into the group");
        assert!(s.tree().iter().filter(|r| r.depth == 1).count() == 2);
        assert_depth_first(&s.nodes);

        let other = s.instances.iter().position(|i| i.model_index == 1).unwrap();
        let nodes = s.with_node_moved_into(s.instances[other].node.unwrap(), None).unwrap();
        apply(&mut s, nodes);
        assert_eq!(world_of(&s), before, "back to the root");
        assert_depth_first(&s.nodes);
    }

    #[test]
    fn a_group_cannot_go_into_itself() {
        let mut s = scene();
        let nodes = s.with_grouped(&[0, 1]).unwrap();
        apply(&mut s, nodes);
        let group = s.tree()[0].node;
        assert_eq!(s.with_node_moved_into(group, Some(group)), Err(GraphError::IntoItself));
    }

    #[test]
    fn turning_a_group_turns_its_contents_around_the_centre() {
        let mut s = scene();
        let nodes = s.with_grouped(&[0, 1]).unwrap();
        apply(&mut s, nodes);
        let group = s.tree()[0].node;
        let held = s.instances_under(group);
        assert_eq!(held.len(), 2);
        let centre = IVec3::new(100, 0, 0);
        let before = world_of(&s);
        let nodes = s.with_node_turned(group, centre, 2).unwrap();
        apply(&mut s, nodes);
        let quarter = quarter_turn(2);
        for ((_, b), (_, a)) in before.iter().zip(world_of(&s).iter()) {
            assert_eq!(a.rotation, quarter * b.rotation);
            let expected = (quarter * (b.translation - centre).as_vec3()).round().as_ivec3() + centre;
            assert_eq!(a.translation, expected);
        }
    }

    #[test]
    fn quarter_turns_are_proper_and_four_make_a_whole() {
        for axis in 0..3 {
            let q = quarter_turn(axis);
            assert!((q.determinant() - 1.0).abs() < 1e-6, "a rotation, not a mirror");
            let mut m = Mat3::IDENTITY;
            for _ in 0..4 {
                m = q * m;
            }
            assert_eq!(m, Mat3::IDENTITY);
            // The axis itself stays put.
            let mut a = glam::Vec3::ZERO;
            a[axis] = 1.0;
            assert_eq!(q * a, a);
        }
        // Same sense as the model operation: about z, +x goes to +y.
        assert_eq!(quarter_turn(2) * glam::Vec3::X, glam::Vec3::Y);
        assert_eq!(quarter_turn(0) * glam::Vec3::Y, glam::Vec3::Z);
        assert_eq!(quarter_turn(1) * glam::Vec3::Z, glam::Vec3::X);
    }

    #[test]
    fn turning_about_any_axis_round_trips_through_the_file_encoding() {
        let mut s = scene();
        for axis in 0..3 {
            let before = s.instances[1].transform;
            for _ in 0..4 {
                let nodes = s.with_instance_turned(1, axis).unwrap();
                apply(&mut s, nodes);
            }
            assert_eq!(s.instances[1].transform, before, "axis {axis}");
        }
    }

    #[test]
    fn groups_above_lists_containers_outermost_first() {
        let mut s = scene();
        let nodes = s.with_grouped(&[0, 1]).unwrap();
        apply(&mut s, nodes);
        let outer = s.tree()[0].node;
        assert_eq!(s.groups_above(0), vec![outer]);
        // A group inside it, around the first instance only.
        let nodes = s.with_grouped(&[0]).unwrap();
        apply(&mut s, nodes);
        let outer = s.tree()[0].node;
        let path = s.groups_above(0);
        assert_eq!(path.len(), 2);
        assert_eq!(path[0], outer);
        assert_eq!(s.groups_above(1), vec![outer]);
    }

    #[test]
    fn frames_of_a_shape_and_keys_of_a_transform() {
        let mut s = scene();
        let nodes = s.with_shape_frame(0, 1, 3).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances[0].model_index, 0, "frame 0 keeps its model");
        s.frame = 5;
        s.rebuild();
        assert_eq!(s.instances[0].model_index, 1, "from frame 3 on, the new one");

        // Moving at frame 5 keys the transform there; removing the key undoes it.
        let before = s.instances[0].transform.translation;
        let nodes = s.with_instance_moved(0, IVec3::X).unwrap();
        apply(&mut s, nodes);
        s.frame = 0;
        s.rebuild();
        assert_eq!(s.instances[0].transform.translation, before, "frame 0 unchanged");
        s.frame = 5;
        s.rebuild();
        assert_eq!(s.instances[0].transform.translation, before + IVec3::X);
        let node = s.instances[0].node.unwrap();
        let nodes = s.with_key_removed(node).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances[0].transform.translation, before);
    }

    /// Three instances named `b`, `a` and `C`, in that order, at the root.
    fn named() -> Scene {
        let mut s = scene();
        let nodes = s.with_instance_duplicated(0, IVec3::new(10, 0, 0)).unwrap();
        apply(&mut s, nodes);
        for (i, name) in ["b", "a", "C"].iter().enumerate() {
            let node = s.instances[i].node.unwrap();
            let nodes = s.with_node_attribute(node, "_name", Some(name)).unwrap();
            apply(&mut s, nodes);
        }
        s
    }

    fn names(s: &Scene, group: Option<i32>) -> Vec<String> {
        s.children_of(group)
            .into_iter()
            .map(|n| match find(&s.nodes, n) {
                Some(SceneNode::Transform(t)) => t.attributes.get("_name").unwrap_or("").to_owned(),
                _ => String::new(),
            })
            .collect()
    }

    #[test]
    fn sorting_orders_by_name_and_a_sorted_list_reverses() {
        let mut s = named();
        let nodes = s.with_children_sorted(None, &[]).unwrap();
        apply(&mut s, nodes);
        assert_eq!(names(&s, None), ["C", "a", "b"], "byte order: capitals first");
        assert_depth_first(&s.nodes);
        let nodes = s.with_children_sorted(None, &[]).unwrap();
        apply(&mut s, nodes);
        assert_eq!(names(&s, None), ["b", "a", "C"], "already sorted: reversed");
    }

    #[test]
    fn sorting_a_selection_keeps_the_others_in_place() {
        let mut s = named();
        // `b` (first) and `C` (last) picked: they swap, `a` stays between.
        let picked = [s.instances[0].node.unwrap(), s.instances[2].node.unwrap()];
        let nodes = s.with_children_sorted(None, &picked).unwrap();
        apply(&mut s, nodes);
        assert_eq!(names(&s, None), ["C", "a", "b"]);
        // One picked is not a selection to sort: everything is.
        let mut s = named();
        let nodes = s.with_children_sorted(None, &picked[..1]).unwrap();
        apply(&mut s, nodes);
        assert_eq!(names(&s, None), ["C", "a", "b"]);
    }

    #[test]
    fn a_path_runs_from_the_outermost_group_down() {
        let mut s = scene();
        let nodes = s.with_grouped(&[0, 1]).unwrap();
        apply(&mut s, nodes);
        let nodes = s.with_grouped(&[0]).unwrap();
        apply(&mut s, nodes);
        let outer = s.tree()[0].node;
        let inner = s.groups_above(0)[1];
        let leaf = s.instances[0].node.unwrap();
        assert_eq!(s.path_to(leaf), vec![outer, inner, leaf]);
        assert_eq!(s.children_of(None), vec![outer]);
        assert!(s.nodes_under(outer).contains(&leaf));
        let nodes = s.with_nodes_attribute(&[inner, leaf], "_hidden", Some("1")).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.tree().iter().filter(|r| r.hidden).count(), 2);
    }

    #[test]
    fn packing_makes_frames_and_unpacking_gives_them_back() {
        let mut s = scene();
        let nodes = s.with_instance_duplicated(0, IVec3::new(10, 0, 0)).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances.len(), 3);
        let places: Vec<Transform> = s.instances.iter().map(|i| i.transform).collect();
        let models: Vec<usize> = s.instances.iter().map(|i| i.model_index).collect();

        // Packed with their placements: one instance, a frame each.
        let nodes = s.with_packed(&[0, 1, 2], true, true).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances.len(), 1);
        assert_depth_first(&s.nodes);
        for (f, (&model, place)) in models.iter().zip(&places).enumerate() {
            s.frame = f as u32;
            s.rebuild();
            assert_eq!(s.instances[0].model_index, model, "frame {f}");
            assert_eq!(s.instances[0].transform, *place, "frame {f}");
        }
        let shape = s.nodes.iter().find_map(|n| match n {
            SceneNode::Shape(sh) => Some(sh),
            _ => None,
        });
        assert_eq!(shape.unwrap().attributes.get("_loop"), Some("1"));

        // Unpacked: back to three, each where its frame had it.
        s.frame = 0;
        s.rebuild();
        let nodes = s.with_unpacked(0).unwrap();
        apply(&mut s, nodes);
        assert_eq!(s.instances.len(), 3);
        let mut back: Vec<(usize, IVec3)> = s.instances.iter().map(|i| (i.model_index, i.transform.translation)).collect();
        let mut want: Vec<(usize, IVec3)> = models.iter().zip(&places).map(|(&m, p)| (m, p.translation)).collect();
        back.sort_by_key(|k| (k.1.x, k.1.y, k.1.z));
        want.sort_by_key(|k| (k.1.x, k.1.y, k.1.z));
        assert_eq!(back, want);
        assert_depth_first(&s.nodes);
    }

    #[test]
    fn packing_without_xform_keeps_the_first_place() {
        let mut s = scene();
        let first = s.instances[0].transform;
        let nodes = s.with_packed(&[0, 1], false, false).unwrap();
        apply(&mut s, nodes);
        s.frame = 1;
        s.rebuild();
        assert_eq!(s.instances[0].transform, first, "no keys: every frame at the first's place");
        assert!(s.with_unpacked(0).is_ok());
    }
}
