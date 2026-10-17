//! Edits as values, so undo is a property of the type.
//!
//! the reference editor does this with ~130 C++ classes deriving from `the reference writer`,
//! each implementing `DoIt`/`Undo`/`Redo`. An enum gives the same structure
//! with one advantage: `match` is exhaustive, so adding a command the compiler
//! cannot undo is a build error rather than a bug found later.

use crate::{Document, Volume};
use glam::IVec3;
use vox_format::{Color, Dict, Layer, Material, SceneNode};

/// A single reversible change to a [`Document`].
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Writes palette indices at specific cells of one model, remembering
    /// what was there.
    SetVoxels {
        /// Index into the scene's model list.
        model: usize,
        /// `(position, previous, next)` per cell.
        changes: Vec<(IVec3, u8, u8)>,
    },
    /// Swaps a whole model for another: resizing, rotating, mirroring and
    /// cropping change its shape, which a list of cell writes cannot express.
    ReplaceModel {
        model: usize,
        before: Volume,
        after: Volume,
    },
    /// Changes one palette colour.
    SetColor { index: u8, before: Color, after: Color },
    /// Changes the material of one palette index; `None` is plain diffuse.
    SetMaterial {
        index: u8,
        before: Option<Material>,
        after: Option<Material>,
    },
    /// Swaps the scene graph: moving, turning, adding or removing instances.
    SetGraph {
        before: Vec<SceneNode>,
        after: Vec<SceneNode>,
    },
    /// Replaces the render settings. Dragging a slider merges into one step.
    SetRender { before: Vec<Dict>, after: Vec<Dict> },
    /// Replaces the `META` objects: the animation range.
    SetMeta { before: Vec<Dict>, after: Vec<Dict> },
    /// Reorders the palette as shown: display slot to colour index (the
    /// file's `IMAP`). Colours and voxels stay as they are.
    SetPaletteOrder { before: Box<[u8; 256]>, after: Box<[u8; 256]> },
    /// Replaces the saved cameras: a view stored in a slot.
    SetCameras { before: Vec<vox_format::Camera>, after: Vec<vox_format::Camera> },
    /// Replaces the layer table: names, colours, visibility, new layers.
    SetLayers { before: Vec<Layer>, after: Vec<Layer> },
    /// Appends a model to the model list; undone by removing it again.
    PushModel { volume: Volume },
    /// Several commands as one undo step, applied in order and reverted in
    /// reverse: a new model and the instance placing it, or a crop and the
    /// move that keeps the voxels where they were.
    Batch(Vec<Command>),
}

impl Command {
    /// Builds a command from intended writes, capturing the previous values.
    ///
    /// Cells whose value would not change are dropped, so a stroke that
    /// repaints the same colour does not land on the undo stack as a no-op.
    /// So are cells outside the model, which a write cannot reach.
    pub fn set_voxels(
        document: &Document,
        model: usize,
        writes: impl IntoIterator<Item = (IVec3, u8)>,
    ) -> Self {
        let changes = match document.scene.models.get(model) {
            Some(volume) => {
                let [sx, sy, sz] = volume.size();
                let inside = |p: IVec3| {
                    p.x >= 0
                        && p.y >= 0
                        && p.z >= 0
                        && (p.x as u32) < sx
                        && (p.y as u32) < sy
                        && (p.z as u32) < sz
                };
                // A stroke may name the same cell twice; the last write wins
                // and the first previous value is the one to restore. A bit
                // per cell of the model says which were named already; only
                // a repeat looks for its entry, through a map built on the
                // first one -- a whole-model edit names each cell once.
                let index = |p: IVec3| p.x as usize + sx as usize * (p.y as usize + sy as usize * p.z as usize);
                let mut seen = vec![0u64; (sx as usize * sy as usize * sz as usize).div_ceil(64)];
                let mut at: Option<std::collections::HashMap<IVec3, usize>> = None;
                let mut changes: Vec<(IVec3, u8, u8)> = Vec::new();
                for (p, next) in writes {
                    if !inside(p) {
                        continue;
                    }
                    let i = index(p);
                    if seen[i / 64] & (1 << (i % 64)) == 0 {
                        seen[i / 64] |= 1 << (i % 64);
                        if let Some(at) = &mut at {
                            at.insert(p, changes.len());
                        }
                        changes.push((p, volume.get_i(p.x, p.y, p.z), next));
                    } else {
                        let at = at.get_or_insert_with(|| changes.iter().enumerate().map(|(k, c)| (c.0, k)).collect());
                        changes[at[&p]].2 = next;
                    }
                }
                changes.retain(|&(_, previous, next)| previous != next);
                changes
            }
            None => Vec::new(),
        };
        Self::SetVoxels { model, changes }
    }

    /// The cells where `after` differs from model `model`, as one
    /// [`Command::SetVoxels`] -- only those `keep` admits. The two must be
    /// the same size. Compares the cells directly: the way to record an
    /// edit of a whole model.
    pub fn diff(document: &Document, model: usize, after: &Volume, keep: impl Fn(IVec3) -> bool) -> Self {
        let changes = match document.scene.models.get(model) {
            Some(before) if before.size() == after.size() => {
                let [sx, sy, _] = before.size().map(|s| s as usize);
                before
                    .cells()
                    .iter()
                    .zip(after.cells())
                    .enumerate()
                    .filter(|(_, (b, a))| b != a)
                    .map(|(i, (&b, &a))| (IVec3::new((i % sx) as i32, (i / sx % sy) as i32, (i / (sx * sy)) as i32), b, a))
                    .filter(|&(p, _, _)| keep(p))
                    .collect()
            }
            _ => Vec::new(),
        };
        Self::SetVoxels { model, changes }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::SetVoxels { changes, .. } => changes.is_empty(),
            Self::ReplaceModel { before, after, .. } => before == after,
            Self::SetColor { before, after, .. } => before == after,
            Self::SetMaterial { before, after, .. } => before == after,
            Self::SetGraph { before, after } => before == after,
            Self::SetRender { before, after } => before == after,
            Self::SetLayers { before, after } => before == after,
            Self::SetMeta { before, after } => before == after,
            Self::SetPaletteOrder { before, after } => before == after,
            Self::SetCameras { before, after } => before == after,
            Self::PushModel { .. } => false,
            Self::Batch(commands) => commands.iter().all(Command::is_empty),
        }
    }

    pub fn apply(&self, document: &mut Document) {
        match self {
            Self::SetVoxels { model, changes } => {
                if let Some(volume) = document.model_mut(*model) {
                    volume.set_many(changes.iter().map(|&(p, _, next)| (p, next)));
                }
            }
            Self::ReplaceModel { model, after, .. } => {
                if let Some(volume) = document.model_mut(*model) {
                    *volume = after.clone();
                }
            }
            Self::SetColor { index, after, .. } => document.palette.set_color(*index, *after),
            Self::SetMaterial { index, after, .. } => document.put_material(*index, after.clone()),
            Self::SetGraph { after, .. } => {
                document.scene.nodes = after.clone();
                document.scene.rebuild();
            }
            Self::SetRender { after, .. } => document.render = after.clone(),
            Self::SetLayers { after, .. } => document.layers = after.clone(),
            Self::SetMeta { after, .. } => document.meta = after.clone(),
            Self::SetPaletteOrder { after, .. } => document.palette_order = after.clone(),
            Self::SetCameras { after, .. } => document.cameras = after.clone(),
            Self::PushModel { volume } => {
                document.scene.models.push(volume.clone());
                document.scene.rebuild();
            }
            Self::Batch(commands) => {
                for c in commands {
                    c.apply(document);
                }
            }
        }
    }

    pub fn revert(&self, document: &mut Document) {
        match self {
            Self::SetVoxels { model, changes } => {
                if let Some(volume) = document.model_mut(*model) {
                    // Reverse order, so overlapping writes unwind correctly.
                    volume.set_many(changes.iter().rev().map(|&(p, previous, _)| (p, previous)));
                }
            }
            Self::ReplaceModel { model, before, .. } => {
                if let Some(volume) = document.model_mut(*model) {
                    *volume = before.clone();
                }
            }
            Self::SetColor { index, before, .. } => document.palette.set_color(*index, *before),
            Self::SetMaterial { index, before, .. } => {
                document.put_material(*index, before.clone())
            }
            Self::SetGraph { before, .. } => {
                document.scene.nodes = before.clone();
                document.scene.rebuild();
            }
            Self::SetRender { before, .. } => document.render = before.clone(),
            Self::SetLayers { before, .. } => document.layers = before.clone(),
            Self::SetMeta { before, .. } => document.meta = before.clone(),
            Self::SetPaletteOrder { before, .. } => document.palette_order = before.clone(),
            Self::SetCameras { before, .. } => document.cameras = before.clone(),
            Self::PushModel { .. } => {
                document.scene.models.pop();
                document.scene.rebuild();
            }
            Self::Batch(commands) => {
                for c in commands.iter().rev() {
                    c.revert(document);
                }
            }
        }
    }

    /// Merges `next` into this command if they belong to one gesture.
    ///
    /// A brush drag fires one command per frame, and a colour slider one per
    /// frame it moves; without this the undo stack fills with tiny steps and
    /// one Cmd+Z barely undoes anything. the reference editor calls the same idea
    /// `CombinePrevCmd`.
    pub fn absorb(&mut self, next: &Command) -> bool {
        match (self, next) {
            (
                Self::SetVoxels { model, changes },
                Self::SetVoxels { model: other, changes: more },
            ) if model == other => {
                changes.extend(more.iter().copied());
                true
            }
            (
                Self::SetColor { index, after, .. },
                Self::SetColor { index: other, after: newer, .. },
            ) if index == other => {
                *after = *newer;
                true
            }
            (
                Self::SetMaterial { index, after, .. },
                Self::SetMaterial { index: other, after: newer, .. },
            ) if index == other => {
                after.clone_from(newer);
                true
            }
            (Self::SetRender { after, .. }, Self::SetRender { after: newer, .. }) => {
                after.clone_from(newer);
                true
            }
            (Self::SetMeta { after, .. }, Self::SetMeta { after: newer, .. }) => {
                after.clone_from(newer);
                true
            }
            // Typing a layer name: one step per word, not per letter.
            (Self::SetLayers { after, .. }, Self::SetLayers { after: newer, .. }) => {
                after.clone_from(newer);
                true
            }
            // Dragging an instance's position field: one step per drag.
            (Self::SetGraph { after, .. }, Self::SetGraph { after: newer, .. }) => {
                after.clone_from(newer);
                true
            }
            _ => false,
        }
    }
}

/// Undo and redo stacks.
#[derive(Debug, Default)]
pub struct History {
    /// Each step with an id naming the document state it leads to.
    undo: Vec<(u64, Command)>,
    redo: Vec<(u64, Command)>,
    /// Commands pushed while this is true may merge into the previous one.
    coalescing: bool,
    /// Source of step ids; 0 is the state before any step.
    last_id: u64,
    /// While a group is open, the undo depth it started at.
    group_start: Option<usize>,
}

impl History {
    /// Writes voxels into one model, capturing their previous values, and
    /// records the edit.
    ///
    /// This exists because the obvious call does not compile:
    ///
    /// ```compile_fail
    /// # use voxel_core::{Command, Document, History};
    /// # use glam::IVec3;
    /// # let (mut history, mut doc) = (History::default(), Document::default());
    /// history.exec(&mut doc, Command::set_voxels(&doc, 0, [(IVec3::ZERO, 1)]));
    /// ```
    ///
    /// The argument borrows the document immutably while the receiver
    /// already holds it mutably. Taking the writes and building the command
    /// inside keeps both borrows apart, so callers never meet the conflict.
    pub fn set_voxels(
        &mut self,
        document: &mut Document,
        model: usize,
        writes: impl IntoIterator<Item = (IVec3, u8)>,
    ) -> usize {
        let command = Command::set_voxels(document, model, writes);
        let count = match &command {
            Command::SetVoxels { changes, .. } => changes.len(),
            _ => 0,
        };
        self.exec(document, command);
        count
    }

    /// Applies a command and records it. Returns whether anything changed.
    pub fn exec(&mut self, document: &mut Document, command: Command) -> bool {
        if command.is_empty() {
            return false;
        }
        command.apply(document);
        self.last_id += 1;

        // A new edit invalidates anything that was undone.
        self.redo.clear();

        if self.coalescing
            && let Some((id, previous)) = self.undo.last_mut()
                && previous.absorb(&command) {
                    // The merged step leads somewhere new: a save taken in
                    // the middle of a drag must not still count as current.
                    *id = self.last_id;
                    return true;
                }
        self.undo.push((self.last_id, command));
        self.coalescing = true;
        true
    }

    /// Runs a command as a gesture of its own, never merged with a stroke
    /// before or after it. Console commands and menu actions use this.
    pub fn exec_alone(&mut self, document: &mut Document, command: Command) -> bool {
        self.end_gesture();
        let changed = self.exec(document, command);
        self.end_gesture();
        changed
    }

    /// Starts a group: every step recorded until [`History::end_group`]
    /// becomes one undo step. For a list of commands run as one action, such
    /// as an agent's batch.
    pub fn begin_group(&mut self) {
        self.end_gesture();
        self.group_start = Some(self.undo.len());
    }

    /// Closes the group, folding its steps into one `Batch`. Returns how
    /// many steps were folded. An undo inside the group that reaches below
    /// where it started moves the start down; a cleared history (a file
    /// opened or a new model) restarts the group on the new document.
    pub fn end_group(&mut self) -> usize {
        self.end_gesture();
        let Some(start) = self.group_start.take() else { return 0 };
        if self.undo.len() <= start + 1 {
            return self.undo.len().saturating_sub(start);
        }
        let steps: Vec<(u64, Command)> = self.undo.drain(start..).collect();
        let id = steps.last().map_or(self.last_id, |(id, _)| *id);
        let count = steps.len();
        let mut commands = Vec::new();
        for (_, c) in steps {
            match c {
                Command::Batch(inner) => commands.extend(inner),
                c => commands.push(c),
            }
        }
        self.undo.push((id, Command::Batch(commands)));
        count
    }

    /// Ends the current gesture, so the next edit starts a fresh undo entry.
    pub fn end_gesture(&mut self) {
        self.coalescing = false;
    }

    pub fn undo(&mut self, document: &mut Document) -> bool {
        self.end_gesture();
        match self.undo.pop() {
            Some((id, c)) => {
                // Undoing past a group's start: the group cannot fold what
                // came before it.
                if let Some(start) = self.group_start
                    && self.undo.len() < start
                {
                    self.group_start = Some(self.undo.len());
                }
                c.revert(document);
                self.redo.push((id, c));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, document: &mut Document) -> bool {
        self.end_gesture();
        match self.redo.pop() {
            Some((id, c)) => {
                c.apply(document);
                self.undo.push((id, c));
                true
            }
            None => false,
        }
    }

    /// Forgets every step, for when a different file is opened. A group
    /// left open stays open, starting over on the new document: a batch
    /// that begins with `new` and then builds is one undo step back to the
    /// empty model, not one per line.
    pub fn clear(&mut self) {
        let grouping = self.group_start.is_some();
        *self = Self::default();
        if grouping {
            self.group_start = Some(0);
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Names the current document state: the id of the step at the top of
    /// the undo stack. Undoing back to a saved state gives that state's
    /// revision again, so the document reads as unchanged.
    pub fn revision(&self) -> u64 {
        self.undo.last().map_or(0, |(id, _)| *id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Instance, Scene, Transform};

    fn doc() -> Document {
        Document {
            scene: Scene {
                models: vec![Volume::new([8, 8, 8]), Volume::new([4, 4, 4])],
                instances: vec![Instance {
                    model_index: 0,
                    transform: Transform::default(),
                    layer: -1,
                    ..Default::default()
                }],
                nodes: Vec::new(),
                frame: 0,
            },
            ..Default::default()
        }
    }

    fn get(d: &Document, model: usize, x: u32, y: u32, z: u32) -> u8 {
        d.scene.models[model].get(x, y, z)
    }

    #[test]
    fn a_group_survives_a_cleared_history() {
        let mut d = doc();
        let mut h = History::default();
        h.begin_group();
        h.set_voxels(&mut d, 0, [(IVec3::new(5, 0, 0), 9)]);
        h.end_gesture();
        h.clear(); // a new document in the middle of the batch
        h.set_voxels(&mut d, 0, [(IVec3::new(1, 0, 0), 2)]);
        h.end_gesture();
        h.set_voxels(&mut d, 0, [(IVec3::new(2, 0, 0), 3)]);
        assert_eq!(h.end_group(), 2);
        assert_eq!(h.undo_depth(), 1, "what was built after the clear is one step");
        h.undo(&mut d);
        assert_eq!(d.scene.models[0].get(1, 0, 0), 0);
        assert_eq!(d.scene.models[0].get(2, 0, 0), 0);
    }

    #[test]
    fn a_group_undoes_as_one_step() {
        let mut d = doc();
        let mut h = History::default();
        h.set_voxels(&mut d, 0, [(IVec3::new(0, 0, 0), 1)]);
        h.end_gesture();
        h.begin_group();
        h.set_voxels(&mut d, 0, [(IVec3::new(1, 0, 0), 2)]);
        h.end_gesture();
        h.set_voxels(&mut d, 0, [(IVec3::new(2, 0, 0), 3)]);
        h.end_gesture();
        let before = h.revision();
        assert_eq!(h.end_group(), 2);
        assert_eq!(h.undo_depth(), 2, "the step before the group stays apart");
        assert_eq!(h.revision(), before, "the folded step leads to the same state");
        h.undo(&mut d);
        assert_eq!(d.scene.models[0].get(1, 0, 0), 0);
        assert_eq!(d.scene.models[0].get(2, 0, 0), 0);
        assert_eq!(d.scene.models[0].get(0, 0, 0), 1);
        h.redo(&mut d);
        assert_eq!(d.scene.models[0].get(2, 0, 0), 3);
    }

    #[test]
    fn undo_restores_the_previous_value() {
        let mut d = doc();
        d.scene.models[0].set(1, 1, 1, 5);
        let mut h = History::default();

        h.set_voxels(&mut d, 0, [(IVec3::new(1, 1, 1), 9)]);
        assert_eq!(get(&d, 0, 1, 1, 1), 9);

        assert!(h.undo(&mut d));
        assert_eq!(get(&d, 0, 1, 1, 1), 5, "should be back to the old colour");

        assert!(h.redo(&mut d));
        assert_eq!(get(&d, 0, 1, 1, 1), 9);
    }

    #[test]
    fn edits_land_on_the_model_they_name() {
        let mut d = doc();
        let mut h = History::default();
        h.set_voxels(&mut d, 1, [(IVec3::new(2, 2, 2), 4)]);
        assert_eq!(get(&d, 1, 2, 2, 2), 4);
        assert_eq!(get(&d, 0, 2, 2, 2), 0, "the other model is untouched");
        h.undo(&mut d);
        assert_eq!(get(&d, 1, 2, 2, 2), 0);
    }

    #[test]
    fn strokes_on_different_models_stay_separate_steps() {
        let mut d = doc();
        let mut h = History::default();
        h.set_voxels(&mut d, 0, [(IVec3::ZERO, 1)]);
        h.set_voxels(&mut d, 1, [(IVec3::ZERO, 1)]);
        assert_eq!(h.undo_depth(), 2, "one undo must never touch two models");
    }

    #[test]
    fn no_op_and_out_of_bounds_writes_never_reach_the_stack() {
        let mut d = doc();
        d.scene.models[0].set(2, 2, 2, 7);
        let mut h = History::default();
        h.set_voxels(&mut d, 0, [(IVec3::new(2, 2, 2), 7)]);
        h.set_voxels(&mut d, 0, [(IVec3::new(-1, 2, 2), 7), (IVec3::new(8, 0, 0), 7)]);
        assert!(!h.can_undo(), "nothing actually changed");
    }

    #[test]
    fn repeated_cells_in_one_command_undo_to_the_original() {
        let mut d = doc();
        let mut h = History::default();
        let p = IVec3::new(3, 3, 3);
        h.set_voxels(&mut d, 0, [(p, 1), (p, 2), (p, 3)]);
        assert_eq!(get(&d, 0, 3, 3, 3), 3);
        h.undo(&mut d);
        assert_eq!(get(&d, 0, 3, 3, 3), 0);
    }

    #[test]
    fn a_gesture_collapses_into_one_undo_entry() {
        let mut d = doc();
        let mut h = History::default();
        for i in 0..5 {
            h.set_voxels(&mut d, 0, [(IVec3::new(i, 0, 0), 3)]);
        }
        assert_eq!(h.undo_depth(), 1, "one drag is one undo");

        h.undo(&mut d);
        for i in 0..5 {
            assert_eq!(get(&d, 0, i, 0, 0), 0, "the whole stroke should be gone");
        }
    }

    #[test]
    fn ending_a_gesture_starts_a_new_entry() {
        let mut d = doc();
        let mut h = History::default();
        h.set_voxels(&mut d, 0, [(IVec3::new(0, 0, 0), 1)]);
        h.end_gesture();
        h.set_voxels(&mut d, 0, [(IVec3::new(1, 0, 0), 1)]);
        assert_eq!(h.undo_depth(), 2);
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut d = doc();
        let mut h = History::default();
        h.set_voxels(&mut d, 0, [(IVec3::new(0, 0, 0), 1)]);
        h.undo(&mut d);
        assert!(h.can_redo());

        h.set_voxels(&mut d, 0, [(IVec3::new(5, 5, 5), 2)]);
        assert!(!h.can_redo(), "redo cannot survive a divergent edit");
    }

    #[test]
    fn undo_on_an_empty_stack_is_harmless() {
        let mut d = doc();
        let mut h = History::default();
        assert!(!h.undo(&mut d));
        assert!(!h.redo(&mut d));
    }

    #[test]
    fn replacing_a_model_undoes_its_size_too() {
        let mut d = doc();
        let mut h = History::default();
        let before = d.scene.models[1].clone();
        let after = Volume::new([2, 9, 3]);
        h.exec_alone(&mut d, Command::ReplaceModel { model: 1, before, after });
        assert_eq!(d.scene.models[1].size(), [2, 9, 3]);
        h.undo(&mut d);
        assert_eq!(d.scene.models[1].size(), [4, 4, 4]);
    }

    #[test]
    fn a_colour_drag_is_one_step_back_to_the_first_colour() {
        let mut d = doc();
        let mut h = History::default();
        let original = d.palette.color(7);
        let mut current = original;
        for r in [10, 20, 30] {
            let next = Color { r, ..current };
            h.exec(&mut d, Command::SetColor { index: 7, before: current, after: next });
            current = next;
        }
        assert_eq!(h.undo_depth(), 1);
        assert_eq!(d.palette.color(7).r, 30);
        h.undo(&mut d);
        assert_eq!(d.palette.color(7), original);
    }

    #[test]
    fn materials_are_added_changed_and_removed_by_undo() {
        let mut d = doc();
        let mut h = History::default();
        let metal = Material {
            id: 0,
            properties: vox_format::Dict(vec![("_type".into(), "_metal".into())]),
        };
        h.exec_alone(&mut d, Command::SetMaterial { index: 12, before: None, after: Some(metal) });
        assert_eq!(d.material(12).map(|m| m.kind()), Some("_metal"));
        assert_eq!(d.material(12).map(|m| m.id), Some(12), "the id follows the slot");
        h.undo(&mut d);
        assert!(d.material(12).is_none());
    }

    #[test]
    fn the_revision_moves_with_every_change() {
        let mut d = doc();
        let mut h = History::default();
        let r0 = h.revision();
        h.set_voxels(&mut d, 0, [(IVec3::ZERO, 1)]);
        let r1 = h.revision();
        h.undo(&mut d);
        assert!(r1 != r0 && h.revision() != r1);
    }

    #[test]
    fn a_new_model_and_its_instance_undo_as_one_step() {
        let file = vox_format::VoxFile::new_single([4, 4, 4]);
        let mut d = Document { scene: Scene::from_file(&file), ..Default::default() };
        let mut h = History::default();
        let mut with_model = d.scene.clone();
        with_model.models.push(Volume::new([2, 2, 2]));
        let nodes = with_model.with_new_instance(1, Transform::default()).unwrap();
        let before = d.scene.nodes.clone();
        h.exec_alone(
            &mut d,
            Command::Batch(vec![
                Command::PushModel { volume: Volume::new([2, 2, 2]) },
                Command::SetGraph { before, after: nodes },
            ]),
        );
        assert_eq!((d.scene.models.len(), d.scene.instances.len()), (2, 2));
        h.undo(&mut d);
        assert_eq!((d.scene.models.len(), d.scene.instances.len()), (1, 1));
        h.redo(&mut d);
        assert_eq!(d.scene.instances.len(), 2);
    }

    #[test]
    fn undoing_back_to_a_save_reads_as_unchanged() {
        let mut d = doc();
        let mut h = History::default();
        h.set_voxels(&mut d, 0, [(IVec3::ZERO, 1)]);
        h.end_gesture();
        let saved = h.revision();
        h.set_voxels(&mut d, 0, [(IVec3::X, 1)]);
        assert_ne!(h.revision(), saved);
        h.undo(&mut d);
        assert_eq!(h.revision(), saved, "back where it was saved");
        h.redo(&mut d);
        assert_ne!(h.revision(), saved);
    }

    #[test]
    fn a_merge_after_a_save_counts_as_a_change() {
        let mut d = doc();
        let mut h = History::default();
        h.set_voxels(&mut d, 0, [(IVec3::ZERO, 1)]);
        let saved = h.revision();
        // Same gesture, so it merges into the saved step.
        h.set_voxels(&mut d, 0, [(IVec3::X, 1)]);
        assert_eq!(h.undo_depth(), 1);
        assert_ne!(h.revision(), saved);
    }

    #[test]
    fn the_animation_range_is_an_undoable_edit() {
        let mut d = doc();
        let mut h = History::default();
        assert_eq!(d.anim_range(), None);
        let command = Command::SetMeta { before: d.meta.clone(), after: d.with_anim_range(2, 30) };
        h.exec_alone(&mut d, command);
        assert_eq!(d.anim_range(), Some((2, 30)));
        h.undo(&mut d);
        assert_eq!(d.anim_range(), None);
    }
}

