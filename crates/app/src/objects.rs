//! Several objects at once: the reference editor's World mode selection
//! (`osel_all`, `osel_inv`, `osel_none`, Shift+click), and what acts on it.
//!
//! In the Move tool, a click picks a model (or the selected group holding
//! it); Shift+click without dragging adds it to the selection or takes it
//! out; Cmd+A, Cmd+I and Cmd+D select all, invert and clear, as the reference editor
//! has them in World mode. A drag on empty space draws a marquee: the
//! objects whose box falls wholly inside it are selected -- Shift adds
//! them, Shift+Alt takes them out, as the reference editor's. A drag on any selected
//! object moves them all,
//! Cmd+drag and X, Y, Z turn them all about their common centre, and
//! Delete removes them. `object all|none|invert|add <i>|remove <i>|list`
//! from the console.
//!
//! The selection holds transform node ids, which moving and turning keep.
//! Adding or removing a node renumbers them all (`graph::compact`), so such
//! an edit, or an undo, clears the selection; the operations that make new
//! objects select those afterwards. With nothing in it, the selection is
//! the current model (or group), as before.

use crate::App;
use eframe::egui;
use glam::{IVec3, Vec3};
use voxel_core::Transform;

/// What an object copy leaves in the system clipboard, so its Paste is
/// recognised as one.
const OBJECTS_MARKER: &str = "Voxferro objects";

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

    /// Every object at the current level, in file order: each instance's
    /// node, or inside an open group, its children.
    fn all_object_nodes(&self) -> Vec<i32> {
        let Some(loaded) = &self.scene else { return Vec::new() };
        let mut out: Vec<i32> = Vec::new();
        for i in 0..loaded.doc.scene.instances.len() {
            if let Some(n) = self.move_target_level(i)
                && !out.contains(&n)
            {
                out.push(n);
            }
        }
        out
    }

    /// The object holding instance `index` at the current level, ignoring
    /// a selected group.
    fn move_target_level(&self, index: usize) -> Option<i32> {
        let scene = &self.scene.as_ref()?.doc.scene;
        match self.open_groups.last() {
            Some(_) => self.move_target(index),
            None => scene.instances.get(index)?.node,
        }
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
                // Typed, the first one starts the selection on its own
                // (Shift+click keeps the current model in it).
                if words[0] == "add" && self.objects.is_empty() {
                    self.objects.extend(node);
                    self.selected = i;
                    return format!("1 objects selected: [{i}]");
                }
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
        if self.objects.is_empty() {
            return format!("no objects selected: object commands act on the current model ({})", self.selected);
        }
        format!("{} objects selected: {:?}", self.object_indices().len(), self.object_indices())
    }

    /// the reference editor's world-editor keys, in the Move tool (see `hotkeys`).
    pub fn object_input(&mut self, ctx: &egui::Context) {
        if !self.moving || self.rendering || ctx.egui_wants_keyboard_input() {
            return;
        }
        use egui::Key;
        let mut lines: Vec<String> = Vec::new();
        ctx.input(|i| {
            let (cmd, shift, alt) = (i.modifiers.command, i.modifiers.shift, i.modifiers.alt);
            let plain = !cmd && !alt;
            let pressed = |k: Key| i.key_pressed(k);
            let mut line = |l: &str| lines.push(l.to_owned());
            if cmd && !shift && pressed(Key::A) {
                line("object all");
            }
            if cmd && !shift && pressed(Key::I) {
                line("object invert");
            }
            if cmd && !shift && pressed(Key::D) {
                line("object none");
            }
            if !cmd && (pressed(Key::Delete) || pressed(Key::Backspace)) {
                line("object delete");
            }
            if cmd && pressed(Key::R) {
                line(if shift { "ungroup" } else { "group" });
            }
            if cmd && !shift && pressed(Key::T) {
                line("object own");
            }
            if cmd && shift && pressed(Key::C) {
                line("object copy ref");
            }
            for event in &i.events {
                match event {
                    egui::Event::Copy if !shift => line("object copy"),
                    egui::Event::Cut => line("object cut"),
                    egui::Event::Paste(text) if text == OBJECTS_MARKER => line("object paste"),
                    _ => {}
                }
            }
            if plain {
                let mut delta = IVec3::ZERO;
                for (key, step) in [
                    (Key::ArrowLeft, -IVec3::X),
                    (Key::ArrowRight, IVec3::X),
                    (Key::ArrowDown, -IVec3::Y),
                    (Key::ArrowUp, IVec3::Y),
                    (Key::PageDown, -IVec3::Z),
                    (Key::PageUp, IVec3::Z),
                ] {
                    if pressed(key) {
                        delta += step;
                    }
                }
                if delta != IVec3::ZERO {
                    line(&format!("object move {} {} {}", delta.x, delta.y, delta.z));
                }
                // Its `orot`: minus and zero a turn one way, plus and nine
                // the other.
                if pressed(Key::Minus) {
                    line("object turn z");
                }
                if pressed(Key::Plus) || pressed(Key::Equals) {
                    line("object turn z -");
                }
                if pressed(Key::Num0) {
                    line("object turn x");
                }
                if pressed(Key::Num9) {
                    line("object turn x -");
                }
                if !shift && pressed(Key::G) {
                    line("object ground");
                }
            }
        });
        for line in lines {
            let message = if line == "group" || line == "ungroup" { self.group_command(&line) } else { self.object_command(&line) };
            // A copy marks the clipboard, so the system's Paste comes back
            // as one (as the voxel clipboard does).
            if line.starts_with("object copy") || line == "object cut" {
                ctx.copy_text(OBJECTS_MARKER.to_owned());
            }
            self.console_log.push(message);
        }
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
        if self.objects.is_empty() {
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
            ["turn", axis, rest @ ..] if rest.is_empty() || rest == ["-"] => {
                let Some(axis) = crate::outline::parse_axis(&[axis]) else { return "object turn x|y|z [-]".to_owned() };
                let Some((_, centre)) = self.object_worlds() else { return "no objects".to_owned() };
                // `-` turns the other way: three quarter turns.
                self.objects_turned(axis, if rest.is_empty() { 1 } else { 3 }, centre)
            }
            ["delete"] => return self.delete_objects(),
            ["transform", "copy"] => return self.copy_transform(),
            ["transform", "paste"] => return self.paste_transform(""),
            ["transform", "paste", m @ ("rot" | "pos")] => return self.paste_transform(m),
            ["transform", "reset"] => return self.paste_transform("reset"),
            ["level", "in"] => return self.level_command(-2),
            ["level", "out"] => return self.level_command(-1),
            ["align", axis, side] => {
                let a = match *axis {
                    "x" => 0,
                    "y" => 1,
                    "z" => 2,
                    _ => return "object align x|y|z -1|0|+1".to_owned(),
                };
                return match side.trim_start_matches('+').parse::<i32>() {
                    Ok(s @ -1..=1) => self.align_objects(a, s),
                    _ => "object align x|y|z -1|0|+1".to_owned(),
                };
            }
            ["ground"] => return self.ground_objects(),
            ["flip", axes] if !axes.is_empty() && axes.chars().all(|c| "xyz".contains(c)) => {
                return self.flip_objects([axes.contains('x'), axes.contains('y'), axes.contains('z')]);
            }
            ["union"] => return self.union_objects(),
            ["intersect"] => return self.intersect_objects(),
            ["operand"] => return self.take_operand(),
            ["subtract"] => return self.apply_operand(false),
            ["replace"] => return self.apply_operand(true),
            // the reference editor's `ocopy frame` copies objects as they are at this
            // frame, without their keys: what `object copy` always does.
            ["copy"] | ["copy", "frame"] => return self.copy_objects(false),
            ["pack", flags @ ..] if flags.iter().all(|f| matches!(*f, "xform" | "loop")) => {
                let indices = self.object_indices();
                if indices.is_empty() {
                    return "select the objects to pack".to_owned();
                }
                let Some(scene) = self.scene_ref() else { return "nothing open".to_owned() };
                let nodes = scene.with_packed(&indices, flags.contains(&"xform"), flags.contains(&"loop"));
                return match self.exec_graph(nodes, false) {
                    Ok(()) => {
                        self.forget_picks();
                        format!("{} objects packed into one, a frame each", indices.len())
                    }
                    Err(e) => format!("error: {e}"),
                };
            }
            ["unpack"] => {
                let indices = self.object_indices();
                let indices = if indices.is_empty() { vec![self.selected] } else { indices };
                // From the last, so the indices still name the same ones.
                self.history.begin_group();
                let mut done = 0;
                for &i in indices.iter().rev() {
                    let nodes = self.scene_ref().map(|s| s.with_unpacked(i));
                    if let Some(nodes) = nodes
                        && self.exec_graph(nodes, false).is_ok()
                    {
                        done += 1;
                    }
                }
                self.history.end_group();
                self.forget_picks();
                return if done > 0 { format!("{done} animated objects unpacked, a model each") } else { "nothing to unpack: no object shows more than one model".to_owned() };
            }
            ["copy", "ref"] => return self.copy_objects(true),
            ["cut"] => {
                let copied = self.copy_objects(true);
                let deleted = self.delete_objects();
                return format!("{copied}; {deleted}");
            }
            ["paste"] => return self.paste_objects(),
            ["own"] => return self.own_models(),
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

const OBJECT_USAGE: &str = "object all|none|invert|add <i>|remove <i>|list | move dx dy dz | turn x|y|z | delete | \
    align x|y|z -1|0|+1 | ground | flip <axes> | union | intersect | operand | subtract | replace | copy [ref] | cut | paste | own | \
    transform copy|paste [rot|pos]|reset | level in|out";

/// A marquee over objects being dragged: its corners, and whether it
/// replaces the selection (0), adds to it (1) or takes from it (-1).
pub struct ObjectMarquee {
    pub from: egui::Pos2,
    pub to: egui::Pos2,
    mode: i8,
}

impl App {
    /// Starts a marquee on empty space in the Move tool.
    pub fn begin_object_marquee(&mut self, at: egui::Pos2, modifiers: egui::Modifiers) -> bool {
        let mode = match (modifiers.shift, modifiers.alt) {
            (true, true) => -1,
            (true, false) => 1,
            _ => 0,
        };
        self.object_marquee = Some(ObjectMarquee { from: at, to: at, mode });
        true
    }

    /// Ends the marquee: the objects wholly inside it, by their model's box
    /// on screen.
    pub fn finish_object_marquee(&mut self, view_proj: glam::Mat4, rect: egui::Rect) {
        let Some(m) = self.object_marquee.take() else { return };
        let area = egui::Rect::from_two_pos(m.from, m.to);
        let visible = self.visibility();
        let Some(scene) = self.scene.as_ref().map(|s| &s.doc.scene) else { return };
        let project = |p: Vec3| {
            let c = view_proj * p.extend(1.0);
            let ndc = c.truncate() / c.w.max(1e-6);
            rect.min + egui::vec2((ndc.x + 1.0) * 0.5 * rect.width(), (1.0 - ndc.y) * 0.5 * rect.height())
        };
        let hits: Vec<usize> = (0..scene.instances.len())
            .filter(|&i| visible.shows(&scene.instances[i]))
            .filter(|&i| {
                let Some((lo, hi)) = crate::object_ops::instance_box(scene, i) else { return false };
                let (lo, hi) = (lo.as_vec3(), hi.as_vec3());
                (0..8).all(|k| {
                    let p = Vec3::new(
                        if k & 1 == 0 { lo.x } else { hi.x },
                        if k & 2 == 0 { lo.y } else { hi.y },
                        if k & 4 == 0 { lo.z } else { hi.z },
                    );
                    area.contains(project(p))
                })
            })
            .collect();
        let nodes: Vec<i32> = hits.iter().filter_map(|&i| self.move_target(i)).collect();
        match m.mode {
            0 => self.objects = nodes,
            1 => {
                if self.objects.is_empty() {
                    self.objects.extend(self.move_target(self.selected));
                }
                for n in nodes {
                    if !self.objects.contains(&n) {
                        self.objects.push(n);
                    }
                }
            }
            _ => self.objects.retain(|n| !nodes.contains(n)),
        }
        if let Some(&first) = hits.first()
            && m.mode >= 0
        {
            self.selected = first;
        }
        let message = self.object_select_command(&["list"]);
        self.console_log.push(message);
    }
}

impl App {
    /// the reference editor's `olevel` (Tab, Shift+Tab). Tab on a selected group
    /// opens it: its children become the objects, and selecting, moving
    /// and the object commands stay inside it. Tab on a selected model
    /// leaves the Move tool to edit it; Tab while editing comes back.
    /// Shift+Tab closes the open group, one level up.
    pub fn level_command(&mut self, step: i32) -> String {
        if step < 0 && step != -2 {
            return match self.open_groups.pop() {
                Some(_) => {
                    self.objects.clear();
                    format!("closed the group; {}", self.level_name())
                }
                None => "already at the top level".to_owned(),
            };
        }
        if !self.moving {
            self.moving = true;
            self.selecting = false;
            return format!("back to the objects; {}", self.level_name());
        }
        // A group picked in the scene tree (or with Alt+Shift+click) opens.
        if let Some(group) = self.selected_group.take() {
            self.open_groups.push(group);
            self.objects.clear();
            return format!("opened the group; {}", self.level_name());
        }
        let Some(scene) = self.scene.as_ref().map(|s| &s.doc.scene) else { return "nothing open".to_owned() };
        let nodes = self.object_nodes();
        let [node] = nodes.as_slice() else { return "select one group or model, then Tab".to_owned() };
        let held = scene.instances_under(*node);
        let is_group = held.len() > 1 || held.first().and_then(|&i| scene.instances.get(i)).is_some_and(|i| i.node != Some(*node));
        if is_group {
            self.open_groups.push(*node);
            self.selected_group = None;
            self.objects.clear();
            format!("opened the group; {}", self.level_name())
        } else {
            // A model: edit it, as the reference editor's Tab into a shape.
            if let Some(&i) = held.first() {
                self.selected = i;
            }
            self.moving = false;
            "editing the model; Tab to come back to the objects".to_owned()
        }
    }

    /// Where the object commands act, for messages and the panel.
    pub fn level_name(&self) -> String {
        match self.open_groups.len() {
            0 => "the top level".to_owned(),
            n => format!("inside {n} group{}", if n == 1 { "" } else { "s" }),
        }
    }

    /// Tab and Shift+Tab, outside text fields.
    pub fn level_input(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() || ctx.memory(|m| m.focused().is_some()) {
            return;
        }
        let (tab, shift) = ctx.input(|i| (i.key_pressed(egui::Key::Tab), i.modifiers.shift));
        if tab {
            let message = self.level_command(if shift { -1 } else { -2 });
            self.console_log.push(message);
        }
    }
}
