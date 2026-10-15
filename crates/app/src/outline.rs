//! The scene outline: groups and instances as a tree, and the layer table.
//!
//! Instances can be picked several at a time (Cmd+click) to be grouped; a
//! group row selects the group itself, which then moves, hides, is renamed
//! or dissolved as a whole. Layers are part of the document: hiding one or
//! renaming it is an undoable edit and is saved with the file.

use crate::App;
use eframe::egui;
use glam::IVec3;
use voxel_core::Command;
use voxel_core::graph::{TreeKind, TreeRow};
use vox_format::{Dict, Layer};

impl App {
    /// The transform node scene commands act on: the selected group, or
    /// the selected instance's own node.
    fn target_node(&self) -> Option<i32> {
        self.selected_group.or_else(|| {
            let loaded = self.scene.as_ref()?;
            loaded.doc.scene.instances.get(self.selected)?.node
        })
    }

    /// After a structural edit node ids are renumbered and instance indices
    /// shift: what was picked by them no longer means the same thing.
    pub(crate) fn forget_picks(&mut self) {
        self.picked.clear();
        self.selected_group = None;
    }

    /// `group [i j ...]`, `group select <n>`, `ungroup`,
    /// `node name <text> | hide | show | move dx dy dz`
    pub fn group_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(loaded) = self.scene.as_ref() else { return "nothing open".to_owned() };
        let scene = &loaded.doc.scene;
        let (result, structural) = match words.as_slice() {
            ["group", "select", n] => {
                let groups: Vec<i32> =
                    scene.tree().iter().filter(|r| r.kind == TreeKind::Group).map(|r| r.node).collect();
                return match n.parse::<usize>().ok().and_then(|n| groups.get(n)) {
                    Some(&node) => {
                        self.selected_group = Some(node);
                        format!("group {n} selected")
                    }
                    None => format!("there are {} groups", groups.len()),
                };
            }
            ["group", rest @ ..] => {
                let mut indices: Vec<usize> = rest.iter().filter_map(|w| w.parse().ok()).collect();
                if indices.is_empty() {
                    indices = if self.picked.is_empty() { vec![self.selected] } else { self.picked.iter().copied().collect() };
                }
                (scene.with_grouped(&indices), true)
            }
            ["ungroup"] => match self.selected_group {
                Some(node) => (scene.with_ungrouped(node), true),
                None => return "select a group first".to_owned(),
            },
            ["node", verb, rest @ ..] => {
                let Some(node) = self.target_node() else { return "nothing selected".to_owned() };
                let nodes = match (*verb, rest) {
                    ("name", []) => scene.with_node_attribute(node, "_name", None),
                    ("name", words) => scene.with_node_attribute(node, "_name", Some(&words.join(" "))),
                    ("hide", []) => scene.with_node_attribute(node, "_hidden", Some("1")),
                    ("show", []) => scene.with_node_attribute(node, "_hidden", None),
                    ("move", [x, y, z]) => match (x.parse(), y.parse(), z.parse()) {
                        (Ok(x), Ok(y), Ok(z)) => scene.with_node_moved(node, IVec3::new(x, y, z)),
                        _ => return "node move <dx> <dy> <dz>".to_owned(),
                    },
                    _ => return "node name [text] | hide | show | move dx dy dz".to_owned(),
                };
                (nodes, false)
            }
            _ => return "group [i ...] | ungroup | node name|hide|show|move".to_owned(),
        };
        let verb = words[0];
        match self.exec_graph(result, false) {
            Ok(()) => {
                if structural {
                    self.forget_picks();
                }
                format!("{verb}: done")
            }
            Err(e) => format!("error: {e}"),
        }
    }

    fn exec_layers(&mut self, after: Vec<Layer>, merge: bool) {
        let Some(loaded) = self.scene.as_mut() else { return };
        let command = Command::SetLayers { before: loaded.doc.layers.clone(), after };
        let changed = if merge {
            self.history.exec(&mut loaded.doc, command)
        } else {
            self.history.exec_alone(&mut loaded.doc, command)
        };
        if changed {
            self.mesh_dirty = true;
        }
    }

    /// `layer list | new [name] | hide <n> | show <n> | name <n> <text> |
    /// assign <n>` (assign moves the selected instance there)
    pub fn layer_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let Some(loaded) = self.scene.as_ref() else { return "nothing open".to_owned() };
        let mut layers = loaded.doc.layers.clone();
        let id_of = |w: &str| w.parse::<i32>().ok();
        match words.as_slice() {
            ["list"] => layers
                .iter()
                .map(|l| {
                    let name = l.name().unwrap_or("");
                    format!("{}{} {name}", l.id, if l.is_hidden() { " (hidden)" } else { "" })
                })
                .collect::<Vec<_>>()
                .join("; "),
            ["new", name @ ..] => {
                let id = layers.iter().map(|l| l.id).max().unwrap_or(-1) + 1;
                let mut attributes = Dict::default();
                if !name.is_empty() {
                    attributes.set("_name", name.join(" "));
                }
                layers.push(Layer { id, attributes });
                self.exec_layers(layers, false);
                format!("layer {id} added")
            }
            [verb @ ("hide" | "show"), n] => {
                let Some(layer) = id_of(n).and_then(|id| layers.iter_mut().find(|l| l.id == id)) else {
                    return format!("no layer {n}");
                };
                if *verb == "hide" {
                    layer.attributes.set("_hidden", "1");
                } else {
                    layer.attributes.remove("_hidden");
                }
                self.exec_layers(layers, false);
                format!("layer {n} {verb}n")
            }
            ["name", n, text @ ..] => {
                let Some(layer) = id_of(n).and_then(|id| layers.iter_mut().find(|l| l.id == id)) else {
                    return format!("no layer {n}");
                };
                if text.is_empty() {
                    layer.attributes.remove("_name");
                } else {
                    layer.attributes.set("_name", text.join(" "));
                }
                self.exec_layers(layers, false);
                format!("layer {n} renamed")
            }
            ["assign", n] => {
                let Some(id) = id_of(n).filter(|id| layers.iter().any(|l| l.id == *id)) else {
                    return format!("no layer {n}");
                };
                let nodes = loaded.doc.scene.with_instance_layer(self.selected, id);
                match self.exec_graph(nodes, false) {
                    Ok(()) => format!("instance {} on layer {id}", self.selected),
                    Err(e) => format!("error: {e}"),
                }
            }
            _ => "layer list | new [name] | hide <n> | show <n> | name <n> <text> | assign <n>".to_owned(),
        }
    }

    /// The outline and the actions for what is selected in it.
    pub fn scene_ui(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &self.scene else { return };
        let rows: Vec<TreeRow> = loaded.doc.scene.tree();
        let models: Vec<usize> = loaded.doc.scene.instances.iter().map(|i| i.model_index).collect();

        let mut clicked: Option<(TreeRow, bool)> = None;
        egui::ScrollArea::vertical().max_height(140.0).id_salt("outline").show(ui, |ui| {
            for row in &rows {
                let (label, on) = match row.kind {
                    TreeKind::Group => (
                        format!("{}/", row.name.as_deref().unwrap_or("group")),
                        self.selected_group == Some(row.node),
                    ),
                    TreeKind::Instance(i) => {
                        let model = models.get(i).copied().unwrap_or(0);
                        let name = row.name.clone().unwrap_or_else(|| format!("model {model}"));
                        let on = self.selected_group.is_none() && (i == self.selected || self.picked.contains(&i));
                        (format!("{i}: {name}"), on)
                    }
                };
                let label = if row.hidden { format!("{label}  (hidden)") } else { label };
                ui.horizontal(|ui| {
                    ui.add_space(row.depth as f32 * 12.0);
                    // Groups in bold: egui's default font has no tree glyphs.
                    let mut text = egui::RichText::new(label);
                    if row.kind == TreeKind::Group {
                        text = text.strong();
                    }
                    if row.hidden {
                        text = text.weak();
                    }
                    let response = ui.selectable_label(on, text);
                    if response.clicked() {
                        let additive = ui.input(|i| i.modifiers.command);
                        clicked = Some((row.clone(), additive));
                    }
                });
            }
        });
        if let Some((row, additive)) = clicked {
            match row.kind {
                TreeKind::Group => {
                    self.selected_group = Some(row.node);
                    self.picked.clear();
                }
                TreeKind::Instance(i) => {
                    self.selected_group = None;
                    if additive {
                        if !self.picked.remove(&i) {
                            self.picked.insert(i);
                        }
                        self.picked.insert(self.selected);
                    } else {
                        self.picked.clear();
                    }
                    self.selected = i;
                }
            }
        }

        let node = self.target_node();
        let row = rows.iter().find(|r| Some(r.node) == node).cloned();
        let log = |app: &mut App, message: String| app.console_log.push(message);

        // Name and visibility, for a group or an instance alike.
        if let Some(row) = &row {
            ui.horizontal(|ui| {
                ui.label("Name");
                let mut name = row.name.clone().unwrap_or_default();
                let edit = ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0));
                if edit.lost_focus() && name != row.name.clone().unwrap_or_default() {
                    let message = self.group_command(&format!("node name {name}"));
                    log(self, message);
                }
                let mut visible = !row.hidden;
                if ui.checkbox(&mut visible, "visible").changed() {
                    let message = self.group_command(if visible { "node show" } else { "node hide" });
                    log(self, message);
                }
            });
        }

        if let Some(group) = self.selected_group {
            // A group moves through its own node; everything in it follows.
            let world = self.scene.as_ref().and_then(|s| s.doc.scene.node_world(group));
            if let Some(world) = world {
                ui.horizontal(|ui| {
                    ui.label("Position");
                    let mut p = world.translation;
                    let mut changed = false;
                    for v in [&mut p.x, &mut p.y, &mut p.z] {
                        changed |= ui.add(egui::DragValue::new(v).speed(0.2)).changed();
                    }
                    if changed {
                        let nodes = self
                            .scene
                            .as_ref()
                            .map(|s| s.doc.scene.with_node_moved(group, p - world.translation));
                        if let Some(nodes) = nodes {
                            if let Err(e) = self.exec_graph(nodes, true) {
                                log(self, format!("error: {e}"));
                            }
                        }
                    }
                });
            }
            if ui.small_button("Ungroup").clicked() {
                let message = self.group_command("ungroup");
                log(self, message);
            }
        } else {
            self.instance_ui(ui);
            let count = if self.picked.is_empty() { 1 } else { self.picked.len() };
            if ui
                .small_button(format!("Group ({count})"))
                .on_hover_text("Cmd+click instances to pick several")
                .clicked()
            {
                let message = self.group_command("group");
                log(self, message);
            }
        }
        if ui.small_button("New model").on_hover_text("An empty 16³ model beside the scene").clicked() {
            let message = self.instance_command("instance new");
            log(self, message);
        }
    }

    /// Position, turn, duplicate, delete and layer of the selected instance.
    fn instance_ui(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &self.scene else { return };
        let Some(instance) = loaded.doc.scene.instances.get(self.selected).cloned() else { return };
        let layers: Vec<(i32, String)> = loaded
            .doc
            .layers
            .iter()
            .map(|l| (l.id, l.name().map_or(format!("layer {}", l.id), |n| format!("{} {n}", l.id))))
            .collect();
        let log = |app: &mut App, message: String| app.console_log.push(message);

        ui.add_enabled_ui(instance.node.is_some(), |ui| {
            ui.horizontal(|ui| {
                ui.label("Position");
                let mut p = instance.transform.translation;
                let mut changed = false;
                for v in [&mut p.x, &mut p.y, &mut p.z] {
                    changed |= ui.add(egui::DragValue::new(v).speed(0.2)).changed();
                }
                if changed {
                    let nodes = self.scene.as_ref().map(|s| {
                        let world = voxel_core::Transform { translation: p, ..instance.transform };
                        s.doc.scene.with_instance_placed(self.selected, world)
                    });
                    if let Some(nodes) = nodes {
                        // Merged until the drag ends, like a brush stroke.
                        if let Err(e) = self.exec_graph(nodes, true) {
                            log(self, format!("error: {e}"));
                        }
                    }
                }
            });
            ui.horizontal(|ui| {
                for (label, line) in [("Turn", "instance turn"), ("Duplicate", "instance dup"), ("Delete", "instance delete")] {
                    if ui.small_button(label).clicked() {
                        let message = self.instance_command(line);
                        if line != "instance turn" {
                            self.forget_picks();
                        }
                        log(self, message);
                    }
                }
            });
            let current = layers
                .iter()
                .find(|(id, _)| *id == instance.layer)
                .map_or_else(|| "none".to_owned(), |(_, n)| n.clone());
            egui::ComboBox::from_label("layer")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (id, name) in &layers {
                        if ui.selectable_label(*id == instance.layer, name).clicked() {
                            let message = self.layer_command(&format!("layer assign {id}"));
                            log(self, message);
                        }
                    }
                });
        });
    }

    /// The layer table: visibility, colour, name.
    pub fn layers_ui(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &self.scene else { return };
        let layers = loaded.doc.layers.clone();
        let mut edited: Option<(Vec<Layer>, bool)> = None;
        egui::ScrollArea::vertical().max_height(150.0).id_salt("layers").show(ui, |ui| {
            for (index, layer) in layers.iter().enumerate() {
                ui.horizontal(|ui| {
                    let mut visible = !layer.is_hidden();
                    if ui.checkbox(&mut visible, "").changed() {
                        let mut after = layers.clone();
                        if visible {
                            after[index].attributes.remove("_hidden");
                        } else {
                            after[index].attributes.set("_hidden", "1");
                        }
                        edited = Some((after, false));
                    }
                    let colour = layer
                        .attributes
                        .get_parsed::<u8, 3>("_color")
                        .map_or(egui::Color32::GRAY, |[r, g, b]| egui::Color32::from_rgb(r, g, b));
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, colour);
                    ui.label(format!("{}", layer.id));
                    let mut name = layer.name().unwrap_or_default().to_owned();
                    if ui.add(egui::TextEdit::singleline(&mut name).desired_width(120.0)).changed() {
                        let mut after = layers.clone();
                        if name.is_empty() {
                            after[index].attributes.remove("_name");
                        } else {
                            after[index].attributes.set("_name", name);
                        }
                        // Typing merges until the next gesture.
                        edited = Some((after, true));
                    }
                });
            }
        });
        if let Some((after, merge)) = edited {
            self.exec_layers(after, merge);
        }
        if ui.small_button("New layer").clicked() {
            let message = self.layer_command("layer new");
            self.console_log.push(message);
        }
    }
}
