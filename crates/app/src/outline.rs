//! The scene outline: groups and instances as a tree, and the layer table.
//!
//! Instances can be picked several at a time (Cmd+click; the same object
//! selection as the Move tool's Shift+click) to be grouped; a
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
    pub(crate) fn target_node(&self) -> Option<i32> {
        self.selected_group.or_else(|| {
            let loaded = self.scene.as_ref()?;
            loaded.doc.scene.instances.get(self.selected)?.node
        })
    }

    /// After a structural edit node ids are renumbered and instance indices
    /// shift: what was picked by them no longer means the same thing.
    pub(crate) fn forget_picks(&mut self) {
        self.objects.clear();
        self.open_groups.clear();
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
                    indices = if self.objects.is_empty() { vec![self.selected] } else { self.object_indices() };
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
                    ("turn", axis) => {
                        let Some(axis) = parse_axis(axis) else {
                            return "node turn [x|y|z]".to_owned();
                        };
                        // Around the middle of what it holds: for a group, the
                        // mean of its instances' placements, on the grid.
                        let held = scene.instances_under(node);
                        if held.is_empty() {
                            return "nothing to turn".to_owned();
                        }
                        let sum = held.iter().fold(glam::Vec3::ZERO, |a, &i| a + scene.instances[i].transform.translation.as_vec3());
                        let centre = (sum / held.len() as f32).round().as_ivec3();
                        scene.with_node_turned(node, centre, axis)
                    }
                    ("into", [target]) => {
                        let groups: Vec<i32> =
                            scene.tree().iter().filter(|r| r.kind == TreeKind::Group).map(|r| r.node).collect();
                        let target = match *target {
                            "root" => None,
                            n => match n.parse::<usize>().ok().and_then(|n| groups.get(n)) {
                                Some(&g) => Some(g),
                                None => return format!("there are {} groups", groups.len()),
                            },
                        };
                        let result = scene.with_node_moved_into(node, target);
                        return match self.exec_graph(result, false) {
                            Ok(()) => {
                                self.forget_picks();
                                "moved in the tree".to_owned()
                            }
                            Err(e) => format!("error: {e}"),
                        };
                    }
                    ("move", [x, y, z]) => match (x.parse(), y.parse(), z.parse()) {
                        (Ok(x), Ok(y), Ok(z)) => scene.with_node_moved(node, IVec3::new(x, y, z)),
                        _ => return "node move <dx> <dy> <dz>".to_owned(),
                    },
                    _ => return "node name [text] | hide | show | move dx dy dz | turn [x|y|z] | into <group>|root".to_owned(),
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

    /// `layer list | new [name] | hide <n> | show <n> | toggle <n> | name <n>
    /// <text> | assign <n> | current <n> | only [n] | all | sort | select [n]`
    ///
    /// `assign` moves the selected instance there. `only` shows one layer
    /// (the current one by default) and hides the rest, `all` shows every
    /// layer, `sort` orders the list by name (the reference editor's `lay_vis` and
    /// `lay_sort`), and `select` picks the visible objects of the open group
    /// on that layer (`osel_layer`).
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
            ["toggle", n] => {
                let Some(layer) = id_of(n).and_then(|id| layers.iter_mut().find(|l| l.id == id)) else {
                    return format!("no layer {n}");
                };
                let hidden = layer.is_hidden();
                if hidden {
                    layer.attributes.remove("_hidden");
                } else {
                    layer.attributes.set("_hidden", "1");
                }
                self.exec_layers(layers, false);
                format!("layer {n} {}", if hidden { "shown" } else { "hidden" })
            }
            ["current", n] => match id_of(n).filter(|id| layers.iter().any(|l| l.id == *id)) {
                Some(id) => {
                    self.current_layer = id;
                    format!("current layer {id}")
                }
                None => format!("no layer {n}"),
            },
            ["only", rest @ ..] => {
                let id = rest.first().and_then(|n| id_of(n)).unwrap_or(self.current_layer);
                if !layers.iter().any(|l| l.id == id) {
                    return format!("no layer {id}");
                }
                for layer in &mut layers {
                    if layer.id == id {
                        layer.attributes.remove("_hidden");
                    } else {
                        layer.attributes.set("_hidden", "1");
                    }
                }
                self.exec_layers(layers, false);
                format!("only layer {id} shown")
            }
            ["all"] => {
                for layer in &mut layers {
                    layer.attributes.remove("_hidden");
                }
                self.exec_layers(layers, false);
                "every layer shown".to_owned()
            }
            ["sort"] => {
                // Byte order, as the editor compares: capitals first, unnamed
                // first of all. Ids stay, so nothing on a layer moves.
                layers.sort_by(|a, b| a.name().unwrap_or("").as_bytes().cmp(b.name().unwrap_or("").as_bytes()));
                self.exec_layers(layers, false);
                "layers sorted by name".to_owned()
            }
            ["select", rest @ ..] => {
                let id = rest.first().and_then(|n| id_of(n)).unwrap_or(self.current_layer);
                let scene = &loaded.doc.scene;
                let rows = scene.tree();
                let shown = |layer: i32| layers.iter().find(|l| l.id == layer).is_none_or(|l| !l.is_hidden());
                let picked: Vec<i32> = scene
                    .children_of(self.open_groups.last().copied())
                    .into_iter()
                    .filter(|&n| rows.iter().any(|r| r.node == n && !r.hidden && r.layer == id && shown(r.layer)))
                    .collect();
                let count = picked.len();
                self.selected_group = None;
                if let Some(index) = picked.iter().find_map(|&n| scene.instances_under(n).first().copied()) {
                    self.selected = index;
                }
                self.objects = picked;
                format!("{count} objects on layer {id} selected")
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
            _ => "layer list | new [name] | hide|show|toggle <n> | name <n> <text> | assign <n> | current <n> | only [n] | all | sort | select [n]".to_owned(),
        }
    }

    /// Option+Shift+click in the viewport: selects the outermost group holding
    /// the model under the cursor; each further Option+Shift+click steps one
    /// group inward, then to the model itself, then out again. Returns
    /// whether a model was hit.
    pub fn select_group_at(&mut self, ray: crate::edit::Ray) -> bool {
        let visible = self.visibility();
        let Some(loaded) = &self.scene else { return false };
        let scene = &loaded.doc.scene;
        let Some(pick) = scene.pick(ray.origin, ray.direction, crate::edit::PICK_RANGE, |i| visible.shows(i)) else {
            return false;
        };
        let path = scene.groups_above(pick.instance);
        let next = match self.selected_group.and_then(|g| path.iter().position(|&p| p == g)) {
            // Inside the path: one step in, or past the innermost group to
            // the model.
            Some(k) => path.get(k + 1).copied(),
            // Coming from the model itself, or from elsewhere: outermost.
            None => path.first().copied(),
        };
        self.selected = pick.instance;
        self.objects.clear();
        self.selected_group = next;
        let message = match next {
            Some(group) => format!("group selected ({} models)", scene.instances_under(group).len()),
            None => format!("instance {} selected", pick.instance),
        };
        self.console_log.push(message);
        true
    }

    /// The outline and the actions for what is selected in it.
    pub fn scene_ui(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &self.scene else { return };
        let rows: Vec<TreeRow> = loaded.doc.scene.tree();
        let chosen = self.object_indices();
        let models: Vec<usize> = loaded.doc.scene.instances.iter().map(|i| i.model_index).collect();

        let mut clicked: Option<(TreeRow, bool)> = None;
        // A group opened or closed with its arrow, and a menu command.
        let mut folded: Option<i32> = None;
        let mut menu: Option<&'static str> = None;
        // A row dragged onto a group (or onto the root line) moves there.
        let mut dropped: Option<(i32, Option<i32>)> = None;
        // The scene itself heads the tree, as the drop target for taking
        // things out of every group.
        let root = ui.label(egui::RichText::new("scene").italics());
        if let Some(dragged) = drop_onto(ui, &root) {
            dropped = Some((dragged, None));
        }
        // No scroll area of its own: nested in the panel's, egui sized it to
        // a few rows and hid the rest. The panel scrolls instead.
        ui.vertical(|ui| {
            // Rows under a closed group are skipped: the tree lists depth
            // first, so they are the deeper ones that follow it.
            let mut closed_at: Option<usize> = None;
            for row in &rows {
                if let Some(depth) = closed_at {
                    if row.depth > depth {
                        continue;
                    }
                    closed_at = None;
                }
                let closed = row.kind == TreeKind::Group && self.collapsed.contains(&row.node);
                if closed {
                    closed_at = Some(row.depth);
                }
                let (label, on) = match row.kind {
                    TreeKind::Group => (
                        format!("{}/", row.name.as_deref().unwrap_or("group")),
                        self.selected_group == Some(row.node),
                    ),
                    TreeKind::Instance(i) => {
                        let model = models.get(i).copied().unwrap_or(0);
                        let name = row.name.clone().unwrap_or_else(|| format!("model {model}"));
                        let on = self.selected_group.is_none() && (i == self.selected || chosen.contains(&i));
                        (format!("{i}: {name}"), on)
                    }
                };
                let label = if row.hidden { format!("{label}  (hidden)") } else { label };
                ui.horizontal(|ui| {
                    ui.add_space(row.depth as f32 * 12.0);
                    if row.kind == TreeKind::Group {
                        let arrow = if closed { crate::layout::icon::CARET_RIGHT } else { crate::layout::icon::CARET_DOWN };
                        if ui.small_button(arrow).on_hover_text(if closed { "Expand" } else { "Close" }).clicked() {
                            folded = Some(row.node);
                        }
                    }
                    // Groups in bold: egui's default font has no tree glyphs.
                    let mut text = egui::RichText::new(label);
                    if row.kind == TreeKind::Group {
                        text = text.strong();
                    }
                    if row.hidden {
                        text = text.weak();
                    }
                    let response = ui.selectable_label(on, text).interact(egui::Sense::click_and_drag());
                    if response.clicked() {
                        let additive = ui.input(|i| i.modifiers.command);
                        clicked = Some((row.clone(), additive));
                    }
                    if response.drag_started() {
                        response.dnd_set_drag_payload(TreeDrag(row.node));
                    }
                    if response.secondary_clicked() && !on {
                        // The menu acts on what the row is: select it first.
                        clicked = Some((row.clone(), false));
                    }
                    response.context_menu(|ui| {
                        if let Some(line) = tree_menu_ui(ui) {
                            menu = Some(line);
                        }
                    });
                    if row.kind == TreeKind::Group
                        && let Some(dragged) = drop_onto(ui, &response)
                    {
                        dropped = Some((dragged, Some(row.node)));
                    }
                });
            }
        });
        if let Some((node, target)) = dropped
            && Some(node) != target
        {
            let nodes = self.scene.as_ref().map(|s| s.doc.scene.with_node_moved_into(node, target));
            if let Some(nodes) = nodes {
                let message = match self.exec_graph(nodes, false) {
                    Ok(()) => "moved in the tree".to_owned(),
                    Err(e) => format!("error: {e}"),
                };
                self.forget_picks();
                self.console_log.push(message);
            }
        }
        if let Some(node) = folded
            && !self.collapsed.remove(&node)
        {
            self.collapsed.insert(node);
        }
        if let Some((row, additive)) = clicked {
            match row.kind {
                TreeKind::Group => {
                    self.selected_group = Some(row.node);
                    self.objects.clear();
                }
                TreeKind::Instance(i) => {
                    self.selected_group = None;
                    if additive {
                        self.toggle_object(i);
                    } else {
                        self.objects.clear();
                        self.selected = i;
                    }
                }
            }
        }

        if let Some(line) = menu {
            let message = self.tree_command(line);
            self.console_log.push(message);
        }

        let node = self.target_node();
        let row = rows.iter().find(|r| Some(r.node) == node).cloned();
        let log = |app: &mut App, message: String| app.console_log.push(message);

        // Name and visibility, for a group or an instance alike.
        if let Some(row) = &row {
            ui.horizontal_wrapped(|ui| {
                ui.label("Name");
                let mut name = row.name.clone().unwrap_or_default();
                let edit = ui.add(egui::TextEdit::singleline(&mut name).desired_width(96.0));
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
                ui.horizontal_wrapped(|ui| {
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
                        if let Some(nodes) = nodes
                            && let Err(e) = self.exec_graph(nodes, true)
                        {
                            log(self, format!("error: {e}"));
                        }
                    }
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Turn");
                for a in ["x", "y", "z"] {
                    if ui.small_button(a).on_hover_text(format!("A quarter turn about {a}, around its middle")).clicked() {
                        let message = self.group_command(&format!("node turn {a}"));
                        log(self, message);
                    }
                }
                if ui.small_button("Ungroup").clicked() {
                    let message = self.group_command("ungroup");
                    log(self, message);
                }
            });
        } else {
            self.instance_ui(ui);
            let count = if self.objects.is_empty() { 1 } else { self.object_indices().len() };
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
            ui.horizontal_wrapped(|ui| {
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
            ui.horizontal_wrapped(|ui| {
                ui.label("Turn");
                for a in ["x", "y", "z"] {
                    if ui.small_button(a).on_hover_text(format!("A quarter turn about {a}")).clicked() {
                        let message = self.instance_command(&format!("instance turn {a}"));
                        log(self, message);
                    }
                }
                for (label, line) in [("Duplicate", "instance dup"), ("Delete", "instance delete")] {
                    if ui.small_button(label).clicked() {
                        let message = self.instance_command(line);
                        self.forget_picks();
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
        let mut current = self.current_layer;
        let mut menu: Option<String> = None;
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
                    // The id selects the layer: the one the menu's Hide
                    // others and Select objects mean.
                    let id = ui
                        .selectable_label(current == layer.id, format!("{}", layer.id))
                        .on_hover_text("Select the layer; right-click for its menu");
                    if id.clicked() || id.secondary_clicked() {
                        current = layer.id;
                    }
                    id.context_menu(|ui| {
                        if let Some(line) = layer_menu_ui(ui, layer.id) {
                            menu = Some(line);
                        }
                    });
                    let mut name = layer.name().unwrap_or_default().to_owned();
                    if ui.add(egui::TextEdit::singleline(&mut name).desired_width(96.0)).changed() {
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
        self.current_layer = current;
        if let Some((after, merge)) = edited {
            self.exec_layers(after, merge);
        }
        if let Some(line) = menu {
            let message = self.layer_command(&line);
            self.console_log.push(message);
        }
        if ui.small_button("New layer").clicked() {
            let message = self.layer_command("layer new");
            self.console_log.push(message);
        }
    }
}

/// The outline's menu, the reference editor's items: the console line of the one
/// chosen. On a row's right-click, acting on the selection.
fn tree_menu_ui(ui: &mut egui::Ui) -> Option<&'static str> {
    let items: [(&str, &str, &'static str); 9] = [
        ("Sort by name", "The open group's objects (or the selected ones) by name; again to reverse", "tree sort"),
        ("Close", "Close the selected groups and the groups inside them", "tree close"),
        ("Close inactive", "Close every group but the way to the open one", "tree close inactive"),
        ("Expand", "Open the selected groups and the groups inside them", "tree expand"),
        ("Expand all", "Open every group", "tree expand all"),
        ("Hide", "Hide the selected objects and everything inside them", "tree hide deep"),
        ("Hide others", "Hide everything but the selection and the way to it", "tree hide others"),
        ("Show", "Show the selected objects and everything inside them", "tree show deep"),
        ("Show all", "Show every object", "tree show all"),
    ];
    let mut chosen = None;
    for (i, (label, hint, line)) in items.into_iter().enumerate() {
        if matches!(i, 1 | 5) {
            ui.separator();
        }
        if ui.button(label).on_hover_text(hint).clicked() {
            chosen = Some(line);
            ui.close();
        }
    }
    chosen
}

/// The layer list's menu (`lay_vis`, `lay_sort`, `osel_layer`): the
/// console line of the item chosen, for layer `id`.
fn layer_menu_ui(ui: &mut egui::Ui, id: i32) -> Option<String> {
    let items: [(&str, &str, String); 5] = [
        ("Select objects", "Select the visible objects of the open group on this layer", format!("layer select {id}")),
        ("Hide others", "Show this layer alone", format!("layer only {id}")),
        ("Show all", "Show every layer", "layer all".to_owned()),
        ("Toggle", "Hide or show this layer", format!("layer toggle {id}")),
        ("Sort by name", "Order the list by name", "layer sort".to_owned()),
    ];
    let mut chosen = None;
    for (label, hint, line) in items {
        if ui.button(label).on_hover_text(hint).clicked() {
            chosen = Some(line);
            ui.close();
        }
    }
    chosen
}

/// What a dragged outline row carries: its transform node.
struct TreeDrag(i32);

/// The node dropped on `response` this frame, if any, outlining the target
/// while something hovers over it.
fn drop_onto(ui: &egui::Ui, response: &egui::Response) -> Option<i32> {
    if response.dnd_hover_payload::<TreeDrag>().is_some() {
        ui.painter().rect_stroke(
            response.rect,
            2.0,
            egui::Stroke::new(1.5, egui::Color32::from_rgb(120, 180, 255)),
            egui::StrokeKind::Outside,
        );
    }
    response.dnd_release_payload::<TreeDrag>().map(|d| d.0)
}

/// `x`, `y` or `z` (z when omitted) as an axis index.
pub fn parse_axis(words: &[&str]) -> Option<usize> {
    match words {
        [] | ["z"] => Some(2),
        ["x"] => Some(0),
        ["y"] => Some(1),
        _ => None,
    }
}

