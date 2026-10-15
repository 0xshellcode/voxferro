//! The Scene panel: placed instances, and moving, turning, duplicating,
//! adding and removing them. Every change edits the scene graph, which is
//! saved back as it is.

use crate::App;
use eframe::egui;
use glam::IVec3;
use voxel_core::graph::GraphError;
use voxel_core::{Command, Transform, Volume};
use vox_format::SceneNode;

impl App {
    /// Records a new graph as one undo step. `merge` lets successive edits
    /// of one drag (a position field) collapse into a single step.
    fn exec_graph(&mut self, nodes: Result<Vec<SceneNode>, GraphError>, merge: bool) -> Result<(), String> {
        let nodes = nodes.map_err(|e| e.to_string())?;
        let Some(loaded) = self.scene.as_mut() else { return Err("nothing open".to_owned()) };
        let command = Command::SetGraph { before: loaded.doc.scene.nodes.clone(), after: nodes };
        if merge {
            self.history.exec(&mut loaded.doc, command);
        } else {
            self.history.exec_alone(&mut loaded.doc, command);
        }
        self.after_graph_change();
        Ok(())
    }

    fn after_graph_change(&mut self) {
        let count = self.scene.as_ref().map_or(0, |s| s.doc.scene.instances.len());
        self.selected = self.selected.min(count.saturating_sub(1));
        self.mesh_dirty = true;
    }

    /// `instance list | select <i> | move dx dy dz | place x y z | turn |
    /// dup | delete | new [x y z]`
    pub fn instance_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let Some(loaded) = self.scene.as_ref() else { return "nothing open".to_owned() };
        let scene = &loaded.doc.scene;
        let i = self.selected;
        let ints = |w: &[&str]| -> Option<IVec3> {
            match w {
                [x, y, z] => Some(IVec3::new(x.parse().ok()?, y.parse().ok()?, z.parse().ok()?)),
                _ => None,
            }
        };

        let result = match words.as_slice() {
            ["list"] => {
                return scene
                    .instances
                    .iter()
                    .enumerate()
                    .map(|(n, inst)| {
                        let t = inst.transform.translation;
                        format!("{}{n}: model {} at {} {} {}", if n == i { "*" } else { "" }, inst.model_index, t.x, t.y, t.z)
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
            }
            ["select", n] => match n.parse::<usize>() {
                Ok(n) if n < scene.instances.len() => {
                    self.selected = n;
                    return format!("instance {n} selected");
                }
                _ => return format!("instance select <0-{}>", scene.instances.len().saturating_sub(1)),
            },
            ["move", rest @ ..] => match ints(rest) {
                Some(d) => scene.with_instance_moved(i, d),
                None => return "instance move <dx> <dy> <dz>".to_owned(),
            },
            ["place", rest @ ..] => match (ints(rest), scene.instances.get(i)) {
                (Some(p), Some(inst)) => {
                    scene.with_instance_placed(i, Transform { translation: p, ..inst.transform })
                }
                _ => return "instance place <x> <y> <z>".to_owned(),
            },
            ["turn"] => scene.with_instance_turned(i),
            ["dup"] => scene.with_instance_duplicated(i, self.side_offset(i)),
            ["delete"] => scene.with_instance_removed(i),
            ["new", rest @ ..] => {
                let size = match rest {
                    [] => [16; 3],
                    _ => match ints(rest) {
                        Some(s) if s.min_element() >= 1 && s.max_element() <= 256 => s.as_uvec3().to_array(),
                        _ => return "instance new [x y z], each 1 to 256".to_owned(),
                    },
                };
                return self.new_model(size);
            }
            _ => return "instance list | select <i> | move dx dy dz | place x y z | turn | dup | delete | new [x y z]".to_owned(),
        };
        let what = words[0];
        match self.exec_graph(result, false) {
            Ok(()) => format!("instance {what}: {} placed", self.instance_count()),
            Err(e) => format!("error: {e}"),
        }
    }

    fn instance_count(&self) -> usize {
        self.scene.as_ref().map_or(0, |s| s.doc.scene.instances.len())
    }

    /// Far enough along x for a copy of instance `i` to sit beside it.
    fn side_offset(&self, i: usize) -> IVec3 {
        let Some(loaded) = &self.scene else { return IVec3::X };
        let Some(inst) = loaded.doc.scene.instances.get(i) else { return IVec3::X };
        let Some(volume) = loaded.doc.scene.models.get(inst.model_index) else { return IVec3::X };
        let size = inst.transform.rotation * glam::Vec3::from_array(volume.size().map(|s| s as f32));
        IVec3::new(size.x.abs() as i32 + 2, 0, 0)
    }

    /// Adds an empty model beside the scene, sitting on the ground, with one
    /// instance; both in one undo step.
    fn new_model(&mut self, size: [u32; 3]) -> String {
        let Some(loaded) = self.scene.as_mut() else { return "nothing open".to_owned() };
        let scene = &loaded.doc.scene;
        let right = scene.declared_bounds().map_or(0.0, |(_, max)| max.x);
        let world = Transform {
            translation: IVec3::new(right as i32 + 2 + size[0] as i32 / 2, 0, size[2] as i32 / 2),
            ..Default::default()
        };
        let mut with_model = scene.clone();
        with_model.models.push(Volume::new(size));
        let nodes = match with_model.with_new_instance(with_model.models.len() - 1, world) {
            Ok(n) => n,
            Err(e) => return format!("error: {e}"),
        };
        let command = Command::Batch(vec![
            Command::PushModel { volume: Volume::new(size) },
            Command::SetGraph { before: scene.nodes.clone(), after: nodes },
        ]);
        self.history.exec_alone(&mut loaded.doc, command);
        let count = loaded.doc.scene.instances.len();
        self.selected = count - 1;
        self.after_graph_change();
        format!("new model {}x{}x{}, instance {}", size[0], size[1], size[2], count - 1)
    }

    pub fn scene_ui(&mut self, ui: &mut egui::Ui) {
        let Some(loaded) = &self.scene else { return };
        let rows: Vec<(usize, IVec3, bool)> = loaded
            .doc
            .scene
            .instances
            .iter()
            .map(|i| (i.model_index, i.transform.translation, i.node.is_some()))
            .collect();

        let mut pick = None;
        egui::ScrollArea::vertical().max_height(110.0).id_salt("instances").show(ui, |ui| {
            for (n, (model, t, _)) in rows.iter().enumerate() {
                let label = format!("{n}: model {model}  ({}, {}, {})", t.x, t.y, t.z);
                if ui.selectable_label(n == self.selected, label).clicked() {
                    pick = Some(n);
                }
            }
        });
        if let Some(n) = pick {
            self.selected = n;
        }

        let Some(&(_, translation, placeable)) = rows.get(self.selected) else { return };
        let log = |app: &mut App, message: String| app.console_log.push(message);

        ui.add_enabled_ui(placeable, |ui| {
            ui.horizontal(|ui| {
                ui.label("Position");
                let mut p = translation;
                let mut changed = false;
                for v in [&mut p.x, &mut p.y, &mut p.z] {
                    changed |= ui.add(egui::DragValue::new(v).speed(0.2)).changed();
                }
                if changed {
                    let nodes = self.scene.as_ref().and_then(|s| {
                        let inst = s.doc.scene.instances.get(self.selected)?;
                        Some(s.doc.scene.with_instance_placed(self.selected, Transform { translation: p, ..inst.transform }))
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
                        log(self, message);
                    }
                }
            });
        });
        if ui.small_button("New model").on_hover_text("An empty 16³ model beside the scene").clicked() {
            let message = self.instance_command("instance new");
            log(self, message);
        }
    }
}
