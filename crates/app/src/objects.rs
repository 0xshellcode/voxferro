//! Several objects at once: the reference editor's World mode selection
//! (`osel_all`, `osel_inv`, `osel_none`, Shift+click), and what acts on it.
//!
//! In the Move tool, a click picks a model (or the selected group holding
//! it); Shift+click without dragging adds it to the selection or takes it
//! out; Cmd+A, Cmd+I and Cmd+D select all, invert and clear, as the reference editor
//! has them in World mode. A drag on any selected object moves them all,
//! Cmd+drag and X, Y, Z turn them all about their common centre, and
//! Delete removes them. `object all|none|invert|add <i>|remove <i>|list`
//! from the console.
//!
//! The selection holds transform node ids, not instance indices: those
//! shift whenever an instance is added or removed, while a node keeps its
//! id. With nothing in it, the selection is the current model (or group),
//! as before.

use crate::App;
use eframe::egui;
use glam::{IVec3, Vec3};
use voxel_core::Transform;

impl App {
    /// The transform nodes the object commands act on: the selection, or
    /// the current model's (or selected group's) node. Never one inside
    /// another.
    pub fn object_nodes(&self) -> Vec<i32> {
        let Some(loaded) = &self.scene else { return Vec::new() };
        let scene = &loaded.doc.scene;
        let picked: Vec<i32> = if self.objects.is_empty() {
            self.move_target(self.selected).into_iter().collect()
        } else {
            self.objects.iter().copied().filter(|&n| scene.node_world(n).is_some()).collect()
        };
        scene.outermost(&picked)
    }

    /// The instances under the object nodes.
    pub fn object_indices(&self) -> Vec<usize> {
        let Some(loaded) = &self.scene else { return Vec::new() };
        let mut out: Vec<usize> = self.object_nodes().into_iter().flat_map(|n| loaded.doc.scene.instances_under(n)).collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Adds instance `index`'s node to the selection, or takes it out.
    pub fn toggle_object(&mut self, index: usize) {
        let Some(node) = self.move_target(index) else { return };
        if self.objects.is_empty()
            && let Some(current) = self.move_target(self.selected)
            && current != node
        {
            // The current model was the selection; it stays in it.
            self.objects.push(current);
        }
        match self.objects.iter().position(|&n| n == node) {
            Some(at) => {
                self.objects.remove(at);
            }
            None => {
                self.objects.push(node);
                self.selected = index;
            }
        }
    }

    /// Every instance's node, in file order.
    fn all_object_nodes(&self) -> Vec<i32> {
        let Some(loaded) = &self.scene else { return Vec::new() };
        loaded.doc.scene.instances.iter().filter_map(|i| i.node).collect()
    }

    /// `object all | none | invert | add <i> | remove <i> | list`.
    pub fn object_select_command(&mut self, words: &[&str]) -> String {
        let count = self.scene.as_ref().map_or(0, |s| s.doc.scene.instances.len());
        let index = |w: &str| w.parse::<usize>().ok().filter(|&i| i < count);
        match words {
            ["all"] => self.objects = self.all_object_nodes(),
            ["none"] => self.objects.clear(),
            ["invert"] => {
                let chosen = self.object_nodes();
                self.objects = self.all_object_nodes().into_iter().filter(|n| !chosen.contains(n)).collect();
            }
            ["add", i] | ["remove", i] => {
                let Some(i) = index(i) else { return format!("object add|remove <0-{}>", count.saturating_sub(1)) };
                let node = self.move_target(i);
                let present = node.is_some_and(|n| self.object_nodes().contains(&n));
                if (words[0] == "add") != present {
                    self.toggle_object(i);
                }
            }
            ["list"] => {}
            _ => return "object all|none|invert|add <i>|remove <i>|list".to_owned(),
        }
        if let Some(&first) = self.object_indices().first()
            && !self.object_indices().contains(&self.selected)
        {
            self.selected = first;
        }
        format!("{} objects selected: {:?}", self.object_indices().len(), self.object_indices())
    }

    /// Cmd+A, Cmd+I, Cmd+D and Delete for objects, in the Move tool.
    pub fn object_input(&mut self, ctx: &egui::Context) {
        if !self.moving || ctx.egui_wants_keyboard_input() {
            return;
        }
        let (all, invert, none, delete) = ctx.input(|i| {
            let cmd = i.modifiers.command && !i.modifiers.shift;
            (
                cmd && i.key_pressed(egui::Key::A),
                cmd && i.key_pressed(egui::Key::I),
                cmd && i.key_pressed(egui::Key::D),
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
            )
        });
        let line = if all {
            "object all"
        } else if invert {
            "object invert"
        } else if none {
            "object none"
        } else if delete {
            "object delete"
        } else {
            return;
        };
        let message = self.object_command(line);
        self.console_log.push(message);
    }

    /// The world placements of the object nodes, and the centre of what
    /// they hold.
    pub fn object_worlds(&self) -> Option<(Vec<(i32, Transform)>, IVec3)> {
        let scene = &self.scene.as_ref()?.doc.scene;
        let nodes = self.object_nodes();
        let worlds: Vec<(i32, Transform)> = nodes.iter().filter_map(|&n| Some((n, scene.node_world(n)?))).collect();
        let (lo, hi) = self.bounds_of(&self.object_indices())?;
        Some((worlds, ((lo + hi) * 0.5).round().as_ivec3()))
    }

    /// The object nodes moved by `delta`, as new graph nodes.
    pub fn objects_moved(&self, delta: IVec3) -> Result<Vec<vox_format::SceneNode>, voxel_core::graph::GraphError> {
        let scene = &self.scene.as_ref().ok_or(voxel_core::graph::GraphError::NotPlaceable)?.doc.scene;
        let (worlds, _) = self.object_worlds().ok_or(voxel_core::graph::GraphError::NotPlaceable)?;
        let moved: Vec<(i32, Transform)> =
            worlds.into_iter().map(|(n, w)| (n, Transform { translation: w.translation + delta, ..w })).collect();
        scene.with_worlds(&moved)
    }

    /// The object nodes turned `times` quarter turns about world axis
    /// `axis`, around `centre`.
    pub fn objects_turned(&self, axis: usize, times: u32, centre: IVec3) -> Result<Vec<vox_format::SceneNode>, voxel_core::graph::GraphError> {
        let scene = &self.scene.as_ref().ok_or(voxel_core::graph::GraphError::NotPlaceable)?.doc.scene;
        let (worlds, _) = self.object_worlds().ok_or(voxel_core::graph::GraphError::NotPlaceable)?;
        let mut quarter = glam::Mat3::IDENTITY;
        for _ in 0..times % 4 {
            quarter = voxel_core::graph::quarter_turn(axis) * quarter;
        }
        let turned: Vec<(i32, Transform)> = worlds
            .into_iter()
            .map(|(n, w)| {
                let at = (quarter * (w.translation - centre).as_vec3()).round().as_ivec3() + centre;
                (n, Transform { rotation: quarter * w.rotation, translation: at })
            })
            .collect();
        scene.with_worlds(&turned)
    }

    /// World bounds of every selected object, for the highlight.
    pub fn object_boxes(&self) -> Vec<(Vec3, Vec3)> {
        if !self.moving || self.objects.is_empty() {
            return Vec::new();
        }
        let Some(loaded) = &self.scene else { return Vec::new() };
        self.object_nodes()
            .into_iter()
            .filter_map(|n| self.bounds_of(&loaded.doc.scene.instances_under(n)))
            .collect()
    }
}

impl App {
    /// `object …`: the selection, and what acts on every selected object.
    pub fn object_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let result = match words.as_slice() {
            ["all" | "none" | "invert" | "add" | "remove" | "list", ..] => return self.object_select_command(&words),
            ["move", x, y, z] => match (x.parse(), y.parse(), z.parse()) {
                (Ok(x), Ok(y), Ok(z)) => self.objects_moved(IVec3::new(x, y, z)),
                _ => return "object move <dx> <dy> <dz>".to_owned(),
            },
            ["turn", axis] => {
                let Some(axis) = crate::outline::parse_axis(&[axis]) else { return "object turn x|y|z".to_owned() };
                let Some((_, centre)) = self.object_worlds() else { return "no objects".to_owned() };
                self.objects_turned(axis, 1, centre)
            }
            ["delete"] => return self.delete_objects(),
            _ => return OBJECT_USAGE.to_owned(),
        };
        let n = self.object_indices().len();
        match self.exec_graph(result, false) {
            Ok(()) => format!("{} on {n} objects", words.join(" ")),
            Err(e) => format!("error: {e}"),
        }
    }

    /// Removes every selected object, as one undo step.
    fn delete_objects(&mut self) -> String {
        let mut indices = self.object_indices();
        if indices.is_empty() {
            return "no objects selected".to_owned();
        }
        // Last first, so the indices still to go keep their meaning.
        indices.sort_unstable_by(|a, b| b.cmp(a));
        self.history.begin_group();
        let mut removed = 0;
        for i in &indices {
            let Some(scene) = self.scene.as_ref().map(|s| &s.doc.scene) else { break };
            let nodes = scene.with_instance_removed(*i);
            if self.exec_graph(nodes, false).is_ok() {
                removed += 1;
            }
        }
        self.history.end_group();
        self.objects.clear();
        format!("deleted {removed} objects")
    }
}

const OBJECT_USAGE: &str = "object all|none|invert|add <i>|remove <i>|list | move dx dy dz | turn x|y|z | delete";
