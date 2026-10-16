//! Camera tools: framing, fixed views, projection, and their shortcuts.

use crate::camera::View;
use crate::App;
use eframe::egui;
use glam::Vec3;

impl App {
    /// World bounds of every visible voxel.
    fn bounds_all(&self) -> Option<(Vec3, Vec3)> {
        let loaded = self.scene.as_ref()?;
        let visible = self.visibility();
        let mut bounds = None;
        for instance in loaded.doc.scene.instances.iter().filter(|i| visible.shows(i)) {
            let volume = loaded.doc.scene.models.get(instance.model_index)?;
            if let Some((lo, hi)) = volume.occupied_bounds() {
                let cells = [lo, hi].map(|c| glam::IVec3::new(c[0] as i32, c[1] as i32, c[2] as i32));
                bounds = grow(bounds, cell_box(volume, &instance.transform, cells[0], cells[1]));
            }
        }
        bounds.or_else(|| loaded.doc.scene.declared_bounds())
    }

    /// World bounds of the selected voxels, in the selected instance.
    fn bounds_selection(&self) -> Option<(Vec3, Vec3)> {
        let loaded = self.scene.as_ref()?;
        let selection = self.active_selection()?;
        let instance = loaded.doc.scene.instances.get(self.selected)?;
        if instance.model_index != selection.model {
            return None;
        }
        let volume = loaded.doc.scene.models.get(selection.model)?;
        let mut cells = selection.iter();
        let first = cells.next()?;
        let (lo, hi) = cells.fold((first, first), |(a, b), c| (a.min(c), b.max(c)));
        Some(cell_box(volume, &instance.transform, lo, hi))
    }

    /// World bounds of the given instances' voxels (an empty model counts
    /// by its box).
    pub fn bounds_of(&self, indices: &[usize]) -> Option<(Vec3, Vec3)> {
        let loaded = self.scene.as_ref()?;
        let mut bounds = None;
        for &i in indices {
            let Some(instance) = loaded.doc.scene.instances.get(i) else { continue };
            let Some(volume) = loaded.doc.scene.models.get(instance.model_index) else { continue };
            let (lo, hi) = match volume.occupied_bounds() {
                Some((lo, hi)) => (lo, hi),
                None => ([0; 3], volume.size().map(|s| s.saturating_sub(1))),
            };
            let to_i = |c: [u32; 3]| glam::IVec3::new(c[0] as i32, c[1] as i32, c[2] as i32);
            bounds = grow(bounds, cell_box(volume, &instance.transform, to_i(lo), to_i(hi)));
        }
        bounds
    }

    /// World bounds of the selected group, if one is selected.
    pub fn bounds_group(&self) -> Option<(Vec3, Vec3)> {
        let group = self.selected_group?;
        let held = self.scene.as_ref()?.doc.scene.instances_under(group);
        self.bounds_of(&held)
    }

    /// World bounds of the selected instance's voxels, or its box if empty.
    fn bounds_instance(&self) -> Option<(Vec3, Vec3)> {
        self.bounds_of(&[self.selected])
    }

    /// `camera front|back|left|right|top|bottom|iso | ortho | persp |
    /// frame [all|selection|group|instance]`
    pub fn camera_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let message = match words.as_slice() {
            [name] if View::parse(name).is_some() => {
                self.camera.set_view(View::parse(name).unwrap_or(View::Front));
                format!("view {name}")
            }
            ["ortho"] => {
                self.camera.ortho = true;
                "orthographic".to_owned()
            }
            ["persp"] => {
                self.camera.ortho = false;
                "perspective".to_owned()
            }
            ["frame", what @ ..] => {
                let (name, bounds) = match what {
                    ["all"] => ("all", self.bounds_all()),
                    ["selection"] => ("selection", self.bounds_selection()),
                    ["instance"] => ("instance", self.bounds_instance()),
                    ["group"] => ("group", self.bounds_group()),
                    // The editor's F: the voxel selection if there is one,
                    // else the selected group, else the model being edited.
                    [] => match (self.bounds_selection(), self.bounds_group()) {
                        (Some(b), _) => ("selection", Some(b)),
                        (None, Some(b)) => ("group", Some(b)),
                        (None, None) => ("instance", self.bounds_instance()),
                    },
                    _ => return "camera frame [all|selection|group|instance]".to_owned(),
                };
                match bounds {
                    Some((min, max)) => {
                        self.camera.frame_world_bounds(min, max);
                        format!("framed {name}")
                    }
                    None => format!("nothing to frame ({name})"),
                }
            }
            _ => return "camera front|back|left|right|top|bottom|iso | ortho | persp | frame [all|selection|group|instance]".to_owned(),
        };
        self.path_tracer.reset();
        message
    }

    /// F frames the selection or model, Shift+F everything; 1-7 pick the
    /// fixed views in the order of the panel; O toggles the projection.
    /// Skipped while a text field has focus.
    pub fn camera_input(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let keys = [
            egui::Key::Num1,
            egui::Key::Num2,
            egui::Key::Num3,
            egui::Key::Num4,
            egui::Key::Num5,
            egui::Key::Num6,
            egui::Key::Num7,
        ];
        let mut lines: Vec<String> = Vec::new();
        ctx.input(|i| {
            if i.modifiers.command {
                return;
            }
            if i.key_pressed(egui::Key::F) {
                lines.push(if i.modifiers.shift { "camera frame all" } else { "camera frame" }.to_owned());
            }
            if i.key_pressed(egui::Key::O) {
                lines.push(if self.camera.ortho { "camera persp" } else { "camera ortho" }.to_owned());
            }
            for (key, (_, name)) in keys.iter().zip(View::ALL) {
                if i.key_pressed(*key) {
                    lines.push(format!("camera {name}"));
                }
            }
        });
        for line in lines {
            let message = self.camera_command(&line);
            self.console_log.push(message);
        }
    }

    pub fn camera_ui(&mut self, ui: &mut egui::Ui) {
        let mut lines: Vec<String> = Vec::new();
        ui.horizontal_wrapped(|ui| {
            for (n, (_, name)) in View::ALL.iter().enumerate() {
                if ui.small_button(*name).on_hover_text(format!("key {}", n + 1)).clicked() {
                    lines.push(format!("camera {name}"));
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Frame").on_hover_text("F: the selection, or the model").clicked() {
                lines.push("camera frame".to_owned());
            }
            if ui.small_button("Frame all").on_hover_text("Shift+F").clicked() {
                lines.push("camera frame all".to_owned());
            }
            let mut ortho = self.camera.ortho;
            if ui.checkbox(&mut ortho, "Ortho").on_hover_text("O").changed() {
                lines.push(if ortho { "camera ortho" } else { "camera persp" }.to_owned());
            }
        });
        for line in lines {
            let message = self.camera_command(&line);
            self.console_log.push(message);
        }
    }
}

/// World box of a block of cells of a placed model.
fn cell_box(volume: &voxel_core::Volume, transform: &voxel_core::Transform, lo: glam::IVec3, hi: glam::IVec3) -> (Vec3, Vec3) {
    let a = transform.apply(lo.as_vec3() - volume.pivot());
    let b = transform.apply(hi.as_vec3() + Vec3::ONE - volume.pivot());
    (a.min(b), a.max(b))
}

fn grow(bounds: Option<(Vec3, Vec3)>, (lo, hi): (Vec3, Vec3)) -> Option<(Vec3, Vec3)> {
    Some(match bounds {
        None => (lo, hi),
        Some((a, b)) => (a.min(lo), b.max(hi)),
    })
}
