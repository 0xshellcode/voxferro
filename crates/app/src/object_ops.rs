//! The World-mode object operations, on the selected objects
//! (`objects.rs`):
//!
//! - **Align**: each object moved, on its own, so its box's low
//!   side, centre or high side meets the selection's: `object align x|y|z
//!   -1|0|+1`.
//! - **Ground**: the whole selection moved so its lowest cell
//!   sits on z = 0: `object ground`.
//! - **Flip**: a mirrored placement, not mirrored voxels -- the
//!   selection's box maps onto itself: `object flip x|y|z`.
//! - **Union** and **intersection**: the selected
//!   objects fused into one new model in world cells, at most 256 a side
//!   about the middle; where they overlap, the last selected wins; the
//!   originals go. Intersection keeps only cells every object fills.
//! - **Operand**, **subtract**, **replace**: the
//!   selected objects become the operand -- taken out of the scene and
//!   kept aside -- then subtracting removes from each selected object the
//!   voxels the operand fills, and replacing repaints them in its colours.
//!   The operand stays for further use until the next. A model another
//!   object also shows is copied first, so only the selected one changes.
//! - **Copy**, **copy as reference**, **cut**, **paste**: a copy brings its
//!   own model, so each paste is independent; a reference shows the same
//!   model, so editing one edits all; cut is a reference copy and a delete.
//!   Pasted objects land where they were copied from, and become the
//!   selection.
//! - **Reference to copy**: a selected object sharing its model gets a model
//!   of its own.
//!
//! - **Transform copy and paste**: copy takes the
//!   first selected object's rotation and the centre of its box; paste
//!   gives every selected object that rotation, turning it about its own
//!   centre, and moves that centre onto the copied one. `paste rot` turns
//!   only, `paste pos` moves only, and `reset` turns an object back
//!   upright, in place.
//!
//! Each is one undo step. Groups take part through the models in them.

use crate::App;
use glam::{IVec3, Mat3};
use std::collections::HashMap;
use voxel_core::{Command, Scene, Transform, Volume, world_cell};

/// What `object copy` keeps: a model of its own, or one to reference.
#[derive(Clone)]
pub enum Clip {
    Copy(Volume),
    Reference(usize),
}

/// A box of world cells: low corner and high corner, exclusive.
type CellBox = (IVec3, IVec3);

/// A selected object: its node, world placement and box.
type Placed = (i32, Transform, CellBox);

/// An instance's model box in world cells: low corner and high corner,
/// exclusive.
pub(crate) fn instance_box(scene: &Scene, index: usize) -> Option<(IVec3, IVec3)> {
    let instance = scene.instances.get(index)?;
    let volume = scene.models.get(instance.model_index)?;
    let hi = IVec3::from_array(volume.size().map(|s| s as i32 - 1));
    let (mut lo_w, mut hi_w) = (IVec3::MAX, IVec3::MIN);
    for corner in 0..8 {
        let pick = |bit: i32, a: usize| if corner & bit != 0 { hi[a] } else { 0 };
        let w = world_cell(volume, &instance.transform, IVec3::new(pick(1, 0), pick(2, 1), pick(4, 2)));
        lo_w = lo_w.min(w);
        hi_w = hi_w.max(w);
    }
    Some((lo_w, hi_w + IVec3::ONE))
}

fn union_box(a: Option<(IVec3, IVec3)>, b: (IVec3, IVec3)) -> Option<(IVec3, IVec3)> {
    Some(match a {
        Some((lo, hi)) => (lo.min(b.0), hi.max(b.1)),
        None => b,
    })
}

/// The centre: low side plus half the size, rounded toward 0.
fn centre(lo: IVec3, hi: IVec3) -> IVec3 {
    lo + (hi - lo) / 2
}

/// Every filled voxel of instance `index`, in world cells.
fn instance_cells(scene: &Scene, index: usize) -> Vec<(IVec3, u8)> {
    let Some(instance) = scene.instances.get(index) else { return Vec::new() };
    let Some(volume) = scene.models.get(instance.model_index) else { return Vec::new() };
    let [sx, sy, sz] = volume.size();
    let cells = volume.cells();
    let mut out = Vec::new();
    for (i, &v) in cells.iter().enumerate() {
        if v != 0 {
            let i = i as u32;
            let local = IVec3::new((i % sx) as i32, (i / sx % sy) as i32, (i / (sx * sy)) as i32);
            debug_assert!(local.z < sz as i32);
            out.push((world_cell(volume, &instance.transform, local), v));
        }
    }
    out
}

/// A volume holding `cells`, at most 256 a side about their middle, and
/// the placement that puts it back where they were.
fn volume_of(cells: &HashMap<IVec3, u8>) -> Option<(Volume, Transform)> {
    let (mut lo, mut hi) = (IVec3::MAX, IVec3::MIN);
    for c in cells.keys() {
        lo = lo.min(*c);
        hi = hi.max(*c + IVec3::ONE);
    }
    if cells.is_empty() {
        return None;
    }
    // The clip: 256 a side, about the middle.
    let mid = centre(lo, hi);
    lo = lo.max(mid - IVec3::splat(128));
    hi = hi.min(mid + IVec3::splat(128));
    let size = (hi - lo).as_uvec3().to_array();
    let mut volume = Volume::new(size);
    for (&c, &v) in cells {
        let p = c - lo;
        if p.cmpge(IVec3::ZERO).all() && (c - hi).cmplt(IVec3::ZERO).all() {
            volume.set(p.x as u32, p.y as u32, p.z as u32, v);
        }
    }
    // world_cell(local) = local + 0.5 - pivot + t, floored: t = lo + pivot.
    let t = lo + volume.pivot().as_ivec3();
    Some((volume, Transform { rotation: Mat3::IDENTITY, translation: t }))
}

impl App {
    pub(crate) fn scene_ref(&self) -> Option<&Scene> {
        Some(&self.scene.as_ref()?.doc.scene)
    }

    /// The selected objects' boxes, by node, and their union.
    fn object_boxes_world(&self) -> Option<(Vec<Placed>, CellBox)> {
        let scene = self.scene_ref()?;
        let mut all = None;
        let mut out = Vec::new();
        for node in self.object_nodes() {
            let world = scene.node_world(node)?;
            let b = scene.instances_under(node).into_iter().filter_map(|i| instance_box(scene, i)).fold(None, union_box)?;
            all = union_box(all, b);
            out.push((node, world, b));
        }
        Some((out, all?))
    }

    fn place_objects(&mut self, changes: Vec<(i32, Transform)>, what: &str) -> String {
        if changes.is_empty() {
            return format!("{what}: nothing to move");
        }
        let n = changes.len();
        let Some(scene) = self.scene_ref() else { return "nothing open".to_owned() };
        let nodes = scene.with_worlds(&changes);
        match self.exec_graph(nodes, false) {
            Ok(()) => format!("{what}: {n} objects"),
            Err(e) => format!("error: {e}"),
        }
    }

    /// `object align x|y|z -1|0|+1`.
    pub fn align_objects(&mut self, axis: usize, side: i32) -> String {
        let Some((objects, (s_lo, s_hi))) = self.object_boxes_world() else { return "no objects".to_owned() };
        let changes: Vec<(i32, Transform)> = objects
            .into_iter()
            .filter_map(|(node, world, (lo, hi))| {
                let d = match side {
                    -1 => s_lo[axis] - lo[axis],
                    1 => s_hi[axis] - hi[axis],
                    _ => centre(s_lo, s_hi)[axis] - centre(lo, hi)[axis],
                };
                let mut delta = IVec3::ZERO;
                delta[axis] = d;
                (d != 0).then(|| (node, Transform { translation: world.translation + delta, ..world }))
            })
            .collect();
        self.place_objects(changes, "align")
    }

    /// `object ground`: the selection's lowest cell onto z = 0.
    pub fn ground_objects(&mut self) -> String {
        let Some((objects, (s_lo, _))) = self.object_boxes_world() else { return "no objects".to_owned() };
        let delta = IVec3::new(0, 0, -s_lo.z);
        if delta == IVec3::ZERO {
            return "ground: already on the ground".to_owned();
        }
        let changes = objects.into_iter().map(|(n, w, _)| (n, Transform { translation: w.translation + delta, ..w })).collect();
        self.place_objects(changes, "ground")
    }

    /// `object flip <axes>`: mirrored placements, the selection's box onto
    /// itself.
    pub fn flip_objects(&mut self, axes: [bool; 3]) -> String {
        let Some((objects, (s_lo, s_hi))) = self.object_boxes_world() else { return "no objects".to_owned() };
        let sign = |a: usize| if axes[a] { -1.0 } else { 1.0 };
        let r = Mat3::from_diagonal(glam::Vec3::new(sign(0), sign(1), sign(2)));
        let t = IVec3::from_array([0, 1, 2].map(|a| if axes[a] { s_lo[a] + s_hi[a] } else { 0 }));
        let changes = objects
            .into_iter()
            .map(|(n, w, _)| {
                (n, Transform { rotation: r * w.rotation, translation: (r * w.translation.as_vec3()).round().as_ivec3() + t })
            })
            .collect();
        self.place_objects(changes, "flip")
    }

    /// The selected objects' cells, node by node in selection order.
    fn cells_by_object(&self) -> Vec<Vec<(IVec3, u8)>> {
        let Some(scene) = self.scene_ref() else { return Vec::new() };
        self.object_nodes()
            .into_iter()
            .map(|node| scene.instances_under(node).into_iter().flat_map(|i| instance_cells(scene, i)).collect())
            .collect()
    }

    /// Removes the selected objects and adds `volume` at `place`, one undo
    /// step; the new object is then the selection.
    fn replace_objects_with(&mut self, volume: Volume, place: Transform, what: &str) -> String {
        let mut indices = self.object_indices();
        indices.sort_unstable_by(|a, b| b.cmp(a));
        let voxels = volume.filled_count();
        self.history.begin_group();
        let model = {
            let Some(loaded) = self.scene.as_mut() else { return "nothing open".to_owned() };
            self.history.exec_alone(&mut loaded.doc, Command::PushModel { volume });
            loaded.doc.scene.models.len() - 1
        };
        for i in indices {
            let Some(scene) = self.scene_ref() else { break };
            let nodes = scene.with_instance_removed(i);
            let _ = self.exec_graph(nodes, false);
        }
        let added = self.scene_ref().map(|s| s.with_new_instance(model, place));
        let result = added.map(|n| self.exec_graph(n, false));
        self.history.end_group();
        self.select_model(model);
        match result {
            Some(Ok(())) => format!("{what}: one object of {voxels} voxels"),
            Some(Err(e)) => format!("error: {e}"),
            None => "nothing open".to_owned(),
        }
    }

    /// Makes the instance showing `model` the current one and the selection.
    fn select_model(&mut self, model: usize) {
        let Some(scene) = self.scene_ref() else { return };
        if let Some((i, node)) = scene.instances.iter().enumerate().find(|(_, inst)| inst.model_index == model).map(|(i, inst)| (i, inst.node)) {
            self.selected = i;
            self.objects = node.into_iter().collect();
        }
    }

    /// `object union`.
    pub fn union_objects(&mut self) -> String {
        let objects = self.cells_by_object();
        if objects.len() < 2 && self.object_indices().len() < 2 {
            return "union: select two objects or more".to_owned();
        }
        let mut cells: HashMap<IVec3, u8> = HashMap::new();
        // Last selected first, filling only empty cells: it wins.
        for object in objects.iter().rev() {
            for &(c, v) in object {
                cells.entry(c).or_insert(v);
            }
        }
        match volume_of(&cells) {
            Some((volume, place)) => self.replace_objects_with(volume, place, "union"),
            None => "union: the objects are empty".to_owned(),
        }
    }

    /// `object intersect`.
    pub fn intersect_objects(&mut self) -> String {
        let objects = self.cells_by_object();
        if objects.len() < 2 {
            return "intersect: select two objects or more".to_owned();
        }
        let mut count: HashMap<IVec3, (usize, u8)> = HashMap::new();
        for (k, object) in objects.iter().enumerate() {
            for &(c, v) in object {
                let e = count.entry(c).or_insert((0, v));
                // Counted once an object; the later object's colour.
                if e.0 == k {
                    *e = (k + 1, v);
                }
            }
        }
        let cells: HashMap<IVec3, u8> = count.into_iter().filter(|(_, (n, _))| *n == objects.len()).map(|(c, (_, v))| (c, v)).collect();
        match volume_of(&cells) {
            Some((volume, place)) => self.replace_objects_with(volume, place, "intersect"),
            None => "intersect: the objects do not overlap".to_owned(),
        }
    }

    /// `object operand`: the selection set aside as the operand.
    pub fn take_operand(&mut self) -> String {
        let mut operand = HashMap::new();
        for object in self.cells_by_object() {
            operand.extend(object);
        }
        if operand.is_empty() {
            return "operand: select the objects to subtract or paint with".to_owned();
        }
        let n = self.object_indices().len();
        let voxels = operand.len();
        self.operand = operand;
        let message = self.object_command("object delete");
        if message.starts_with("deleted") {
            format!("operand: {n} objects, {voxels} voxels, set aside; select what to subtract them from or paint")
        } else {
            message
        }
    }

    /// `object subtract` and `object replace`.
    pub fn apply_operand(&mut self, replace: bool) -> String {
        let what = if replace { "replace" } else { "subtract" };
        if self.operand.is_empty() {
            return format!("{what}: no operand; select objects and use operand first");
        }
        let targets = self.object_indices();
        if targets.is_empty() {
            return format!("{what}: no objects selected");
        }
        let mut changed = 0;
        self.history.begin_group();
        for i in targets {
            let Some(scene) = self.scene_ref() else { break };
            let Some(instance) = scene.instances.get(i) else { continue };
            let model = instance.model_index;
            let Some(volume) = scene.models.get(model) else { continue };
            let [sx, sy, _] = volume.size();
            let writes: Vec<(IVec3, u8)> = volume
                .cells()
                .iter()
                .enumerate()
                .filter(|(_, v)| **v != 0)
                .filter_map(|(k, &v)| {
                    let k = k as u32;
                    let local = IVec3::new((k % sx) as i32, (k / sx % sy) as i32, (k / (sx * sy)) as i32);
                    let o = *self.operand.get(&world_cell(volume, &instance.transform, local))?;
                    let next = if replace { o } else { 0 };
                    (next != v).then_some((local, next))
                })
                .collect();
            if writes.is_empty() {
                continue;
            }
            // Shared with another object: this one gets its own copy first.
            let shared = scene.instances.iter().filter(|o| o.model_index == model).count() > 1;
            let model = if shared {
                let copy = volume.clone();
                let frame = scene.frame;
                let Some(loaded) = self.scene.as_mut() else { break };
                self.history.exec_alone(&mut loaded.doc, Command::PushModel { volume: copy });
                let new = loaded.doc.scene.models.len() - 1;
                let nodes = loaded.doc.scene.with_shape_frame(i, new, frame);
                let _ = self.exec_graph(nodes, false);
                new
            } else {
                model
            };
            let Some(loaded) = self.scene.as_mut() else { break };
            let command = Command::set_voxels(&loaded.doc, model, writes);
            self.history.exec_alone(&mut loaded.doc, command);
            changed += 1;
        }
        self.history.end_group();
        self.mesh_dirty = true;
        format!("{what}: {changed} objects changed")
    }

    /// `object copy [ref]`, `object cut`.
    pub fn copy_objects(&mut self, reference: bool) -> String {
        let Some(scene) = self.scene_ref() else { return "nothing open".to_owned() };
        let items: Vec<(Clip, Transform)> = self
            .object_indices()
            .into_iter()
            .filter_map(|i| {
                let inst = scene.instances.get(i)?;
                let clip = if reference { Clip::Reference(inst.model_index) } else { Clip::Copy(scene.models.get(inst.model_index)?.clone()) };
                Some((clip, inst.transform))
            })
            .collect();
        let n = items.len();
        self.object_clip = items;
        format!("copied {n} objects{}", if reference { " as references" } else { "" })
    }

    /// `object paste`: where they were copied from; the pasted objects are
    /// then the selection.
    pub fn paste_objects(&mut self) -> String {
        if self.object_clip.is_empty() {
            return "nothing copied".to_owned();
        }
        let items = self.object_clip.clone();
        let before = self.scene_ref().map_or(0, |s| s.instances.len());
        self.history.begin_group();
        for (clip, place) in items {
            let model = match clip {
                Clip::Reference(m) if self.scene_ref().is_some_and(|s| m < s.models.len()) => m,
                Clip::Reference(_) => continue,
                Clip::Copy(volume) => {
                    let Some(loaded) = self.scene.as_mut() else { break };
                    self.history.exec_alone(&mut loaded.doc, Command::PushModel { volume });
                    loaded.doc.scene.models.len() - 1
                }
            };
            let nodes = self.scene_ref().map(|s| s.with_new_instance(model, place));
            if let Some(nodes) = nodes {
                let _ = self.exec_graph(nodes, false);
            }
        }
        self.history.end_group();
        // New instances go last in the root group: the last ones listed.
        let Some(scene) = self.scene_ref() else { return String::new() };
        let added: Vec<(usize, Option<i32>)> = scene.instances.iter().enumerate().skip(before).map(|(i, inst)| (i, inst.node)).collect();
        self.objects = added.iter().filter_map(|(_, n)| *n).collect();
        if let Some(&(i, _)) = added.first() {
            self.selected = i;
        }
        format!("pasted {} objects", added.len())
    }

    /// `object own`: a selected object sharing its model gets a copy.
    pub fn own_models(&mut self) -> String {
        let targets = self.object_indices();
        let mut done = 0;
        self.history.begin_group();
        for i in targets {
            let Some(scene) = self.scene_ref() else { break };
            let Some(model) = scene.instances.get(i).map(|inst| inst.model_index) else { continue };
            if scene.instances.iter().filter(|o| o.model_index == model).count() < 2 {
                continue;
            }
            let (Some(copy), frame) = (scene.models.get(model).cloned(), scene.frame) else { continue };
            let Some(loaded) = self.scene.as_mut() else { break };
            self.history.exec_alone(&mut loaded.doc, Command::PushModel { volume: copy });
            let new = loaded.doc.scene.models.len() - 1;
            let nodes = loaded.doc.scene.with_shape_frame(i, new, frame);
            if self.exec_graph(nodes, false).is_ok() {
                done += 1;
            }
        }
        self.history.end_group();
        format!("{done} references made copies")
    }
}

impl App {
    /// `object transform copy`.
    pub fn copy_transform(&mut self) -> String {
        let Some((objects, _)) = self.object_boxes_world() else { return "no objects".to_owned() };
        let Some(&(_, world, (lo, hi))) = objects.first() else { return "no objects".to_owned() };
        let centre = (lo + hi).as_vec3() * 0.5;
        self.transform_clip = Some((world.rotation, centre));
        format!("copied the transform: centre {} {} {}", centre.x, centre.y, centre.z)
    }

    /// `object transform paste [rot|pos]`, `object transform reset`.
    pub fn paste_transform(&mut self, mode: &str) -> String {
        let clip = self.transform_clip;
        if clip.is_none() && mode != "reset" {
            return "no transform copied: object transform copy first".to_owned();
        }
        let Some((objects, _)) = self.object_boxes_world() else { return "no objects".to_owned() };
        let changes: Vec<(i32, Transform)> = objects
            .into_iter()
            .filter_map(|(node, world, (lo, hi))| {
                let c = (lo + hi).as_vec3() * 0.5;
                let rotation = match (mode, clip) {
                    ("reset", _) => Mat3::IDENTITY,
                    ("pos", _) => world.rotation,
                    (_, Some((r, _))) => r,
                    _ => return None,
                };
                // Turned about its own centre: q (t - c) + c.
                let q = rotation * world.rotation.transpose();
                let mut t = q * (world.translation.as_vec3() - c) + c;
                if let (true, Some((_, to))) = (mode == "pos" || mode.is_empty(), clip) {
                    t += to - c;
                }
                let next = Transform { rotation, translation: t.round().as_ivec3() };
                (next != world).then_some((node, next))
            })
            .collect();
        self.place_objects(changes, if mode.is_empty() { "paste transform" } else { mode })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_of_cells_goes_back_where_they_were() {
        let cells: HashMap<IVec3, u8> = [(IVec3::new(-3, 5, 2), 7), (IVec3::new(0, 6, 2), 9)].into_iter().collect();
        let (volume, place) = volume_of(&cells).unwrap();
        assert_eq!(volume.size(), [4, 2, 1]);
        for (c, v) in &cells {
            let local = *c - IVec3::new(-3, 5, 2);
            assert_eq!(world_cell(&volume, &place, local), *c);
            assert_eq!(volume.get(local.x as u32, local.y as u32, local.z as u32), *v);
        }
    }

    #[test]
    fn the_centre_rounds_toward_zero() {
        assert_eq!(centre(IVec3::new(0, -5, 1), IVec3::new(3, 0, 2)), IVec3::new(1, -3, 1));
    }
}
