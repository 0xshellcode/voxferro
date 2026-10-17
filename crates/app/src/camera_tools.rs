//! Camera tools: framing, fixed views, projection, and their shortcuts.

use crate::camera::{Projection, View};
use crate::App;
use eframe::egui;
use glam::Vec3;
use voxel_core::saved_camera;

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
            ["set", what, value] => match value.parse::<f32>() {
                Ok(v) => return self.camera_set(what, v),
                Err(_) => return "camera set x|y|z|pitch|yaw|roll|zoom|fov|mouse|keys <value>".to_owned(),
            },
            ["zoom", f] => match f.parse::<f32>() {
                Ok(f) if f > 0.0 => {
                    // Above 1 steps back, below 1 moves in.
                    self.camera.zoom(f);
                    format!("zoom {f}")
                }
                _ => return "camera zoom <factor>".to_owned(),
            },
            ["slots"] => {
                let Some(loaded) = &self.scene else { return "no document".to_owned() };
                let slots = saved_camera::slots(&loaded.doc.cameras);
                let used: Vec<String> = slots
                    .iter()
                    .enumerate()
                    .filter_map(|(i, v)| v.map(|v| format!("{i} ({})", v.projection.name())))
                    .collect();
                if used.is_empty() { "no saved cameras".to_owned() } else { format!("saved cameras: {}", used.join(", ")) }
            }
            ["load", n] => match n.parse::<usize>().ok().filter(|n| *n < saved_camera::SLOTS) {
                Some(slot) => {
                    let Some(loaded) = &self.scene else { return "no document".to_owned() };
                    match saved_camera::slots(&loaded.doc.cameras)[slot] {
                        Some(view) => {
                            self.camera.target = view.target;
                            self.camera.yaw = view.yaw;
                            self.camera.pitch = view.pitch;
                            self.camera.distance = view.distance;
                            self.camera.fov_y = view.fov_y;
                            self.camera.set_projection(view.projection);
                            self.camera.yaw = view.yaw;
                            self.camera.pitch = view.pitch;
                            self.camera.roll = view.roll;
                            self.camera_slot = slot;
                            format!("camera {slot} loaded")
                        }
                        None => format!("camera slot {slot} is empty"),
                    }
                }
                None => return "camera load <0-9>".to_owned(),
            },
            ["save", n] => match n.parse::<usize>().ok().filter(|n| *n < saved_camera::SLOTS) {
                Some(slot) => {
                    let view = saved_camera::SavedView {
                        projection: self.camera.projection,
                        target: self.camera.target,
                        yaw: self.camera.yaw,
                        pitch: self.camera.pitch,
                        distance: self.camera.distance,
                        fov_y: self.camera.fov_y,
                        roll: self.camera.roll,
                    };
                    let Some(loaded) = self.scene.as_mut() else { return "no document".to_owned() };
                    let after = saved_camera::with_slot(&loaded.doc.cameras, slot as i32, &view);
                    let command = voxel_core::Command::SetCameras { before: loaded.doc.cameras.clone(), after };
                    self.history.exec_alone(&mut loaded.doc, command);
                    self.camera_slot = slot;
                    format!("camera {slot} saved")
                }
                None => return "camera save <0-9>".to_owned(),
            },
            ["persp" | "pers"] => {
                self.camera.set_projection(Projection::Perspective);
                "perspective".to_owned()
            }
            ["free"] => {
                self.camera.set_projection(Projection::Free);
                "free camera: drag to look around, W A S D to move, Q E down and up".to_owned()
            }
            ["ortho" | "orth"] => {
                self.camera.set_projection(Projection::Orthographic);
                "orthographic".to_owned()
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
            _ => return "camera front|back|left|right|top|bottom|iso | ortho | persp | zoom <f> | frame [all|selection|group|instance] | slots | save <0-9> | load <0-9>".to_owned(),
        };
        self.path_tracer.reset();
        message
    }

    /// Shift+F frames everything (5 frames the selection or model, in
    /// `hotkeys`); Alt+1 to Alt+7 pick the fixed views in the order of the
    /// panel; O toggles the projection. Skipped while a text field has
    /// focus.
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
        // Free camera: W/S along the view, A/D across it, Q/E down and up,
        // at a speed that scales with the distance to the target; Shift is
        // faster. Held keys move every frame.
        if self.camera.projection == Projection::Free {
            let (forward, right, up, dt, fast) = ctx.input(|i| {
                // Cmd+S and the like are shortcuts, not flying.
                let free = !i.modifiers.command;
                let axis = |plus: egui::Key, minus: egui::Key| {
                    if free { i.key_down(plus) as i32 as f32 - i.key_down(minus) as i32 as f32 } else { 0.0 }
                };
                (
                    axis(egui::Key::W, egui::Key::S),
                    axis(egui::Key::D, egui::Key::A),
                    axis(egui::Key::E, egui::Key::Q),
                    i.stable_dt.min(0.1),
                    i.modifiers.shift,
                )
            });
            if (forward, right, up) != (0.0, 0.0, 0.0) {
                let speed = self.camera.distance.max(8.0) * if fast { 2.0 } else { 0.6 } * dt * self.key_speed;
                self.camera.fly(forward * speed, right * speed, up * speed);
                self.path_tracer.reset();
                ctx.request_repaint();
            }
        }
        ctx.input(|i| {
            if i.modifiers.command {
                return;
            }
            if i.modifiers.shift && i.key_pressed(egui::Key::F) {
                lines.push("camera frame all".to_owned());
            }
            if !i.modifiers.alt && i.key_pressed(egui::Key::O) {
                lines.push(if self.camera.ortho() { "camera persp" } else { "camera ortho" }.to_owned());
            }
            for (key, (_, name)) in keys.iter().zip(View::ALL) {
                if i.modifiers.alt && i.key_pressed(*key) {
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
        self.camera_fields_ui(ui);
        ui.horizontal_wrapped(|ui| {
            for (n, (_, name)) in View::ALL.iter().enumerate() {
                if ui.small_button(*name).on_hover_text(format!("Alt+{}", n + 1)).clicked() {
                    lines.push(format!("camera {name}"));
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Frame").on_hover_text("5: the selection, or the model").clicked() {
                lines.push("camera frame".to_owned());
            }
            if ui.small_button("Frame all").on_hover_text("Shift+F").clicked() {
                lines.push("camera frame all".to_owned());
            }
        });
        ui.horizontal(|ui| {
            for (projection, label, command, hint) in [
                (Projection::Perspective, "Pers", "camera persp", "Perspective (O toggles with orthographic)"),
                (Projection::Free, "Free", "camera free", "Free camera: drag to look, W A S D Q E to move"),
                (Projection::Orthographic, "Orth", "camera ortho", "Orthographic"),
                (Projection::Isometric, "Iso", "camera iso", "Isometric: turns a quarter at a time"),
            ] {
                if ui.selectable_label(self.camera.projection == projection, label).on_hover_text(hint).clicked() {
                    lines.push(command.to_owned());
                }
            }
        });
        // The file's saved cameras, as the editor's ten slots: a filled
        // slot is shown strong; clicking one selects it, Load goes there
        // and Save stores the current view in it.
        let filled = self
            .scene
            .as_ref()
            .map(|s| saved_camera::slots(&s.doc.cameras).map(|v| v.is_some()))
            .unwrap_or([false; saved_camera::SLOTS]);
        ui.horizontal(|ui| {
            ui.label("Slot");
            for (slot, has) in filled.iter().enumerate() {
                let text = egui::RichText::new(slot.to_string());
                let text = if *has { text.strong() } else { text.weak() };
                if ui.selectable_label(self.camera_slot == slot, text).clicked() {
                    self.camera_slot = slot;
                }
            }
        });
        ui.horizontal(|ui| {
            if ui.small_button("Save").on_hover_text("Store this view in the slot (saved in the file)").clicked() {
                lines.push(format!("camera save {}", self.camera_slot));
            }
            let has = filled[self.camera_slot];
            if ui.add_enabled(has, egui::Button::new("Load").small()).clicked() {
                lines.push(format!("camera load {}", self.camera_slot));
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

impl App {
    /// the reference editor's camera panel (`cam.ui`): the point it turns about, its
    /// angles in that editor's convention (pitch, yaw from the front, roll),
    /// zoom, field of view, and how fast the mouse and keys move it.
    fn camera_fields_ui(&mut self, ui: &mut egui::Ui) {
        let mut moved = false;
        let c = &mut self.camera;
        egui::Grid::new("camera fields").num_columns(4).spacing([6.0, 2.0]).show(ui, |ui| {
            let mut field = |ui: &mut egui::Ui, label: &str, v: &mut f32, speed: f64, range: std::ops::RangeInclusive<f32>, hint: &str| {
                ui.label(label).on_hover_text(hint);
                moved |= ui.add(egui::DragValue::new(v).speed(speed).range(range).max_decimals(1)).changed();
            };
            field(ui, "X", &mut c.target.x, 0.5, -10000.0..=10000.0, "The point the camera turns about");
            field(ui, "Pitch", &mut self.camera_angles[0], 0.5, -89.4..=89.4, "Degrees above the horizon");
            ui.end_row();
            field(ui, "Y", &mut c.target.y, 0.5, -10000.0..=10000.0, "The point the camera turns about");
            field(ui, "Yaw", &mut self.camera_angles[1], 0.5, -180.0..=180.0, "Degrees round from the front view");
            ui.end_row();
            field(ui, "Z", &mut c.target.z, 0.5, -10000.0..=10000.0, "The point the camera turns about");
            field(ui, "Roll", &mut self.camera_angles[2], 0.5, -180.0..=180.0, "Degrees about the line of sight");
            ui.end_row();
            field(ui, "Zoom", &mut c.distance, 1.0, 1.0..=100000.0, "Distance from the point, in voxels");
            field(ui, "FOV", &mut self.camera_angles[3], 0.2, 10.0..=120.0, "Field of view, degrees");
            ui.end_row();
            ui.label("Mouse").on_hover_text("How fast a drag turns the view");
            ui.add(egui::DragValue::new(&mut self.mouse_speed).speed(0.01).range(0.1..=3.0).max_decimals(2));
            ui.label("Keys").on_hover_text("How fast W A S D Q E fly, in the free camera");
            ui.add(egui::DragValue::new(&mut self.key_speed).speed(0.01).range(0.01..=100.0).max_decimals(2));
            ui.end_row();
        });
        if moved {
            let [pitch, yaw, roll, fov] = self.camera_angles;
            self.camera.pitch = pitch.to_radians();
            self.camera.yaw = (yaw - 90.0).to_radians();
            self.camera.roll = roll.to_radians();
            self.camera.fov_y = fov.to_radians();
            self.path_tracer.reset();
        } else {
            self.camera_angles = self.panel_angles();
        }
    }

    /// The camera's angles as the panel shows them: pitch, yaw from the
    /// front (-180 to 180), roll, field of view, all in degrees.
    pub fn panel_angles(&self) -> [f32; 4] {
        let yaw = (self.camera.yaw.to_degrees() + 90.0 + 180.0).rem_euclid(360.0) - 180.0;
        [self.camera.pitch.to_degrees(), yaw, self.camera.roll.to_degrees(), self.camera.fov_y.to_degrees()]
    }

    /// `camera set x|y|z|pitch|yaw|roll|zoom|fov|mouse|keys <value>`.
    pub fn camera_set(&mut self, what: &str, value: f32) -> String {
        let mut a = self.panel_angles();
        match what {
            "x" => self.camera.target.x = value,
            "y" => self.camera.target.y = value,
            "z" => self.camera.target.z = value,
            "pitch" => a[0] = value.clamp(-89.4, 89.4),
            "yaw" => a[1] = value,
            "roll" => a[2] = value,
            "fov" => a[3] = value.clamp(10.0, 120.0),
            "zoom" => self.camera.distance = value.max(1.0),
            "mouse" => self.mouse_speed = value.clamp(0.1, 3.0),
            "keys" => self.key_speed = value.clamp(0.01, 100.0),
            _ => return "camera set x|y|z|pitch|yaw|roll|zoom|fov|mouse|keys <value>".to_owned(),
        }
        self.camera.pitch = a[0].to_radians();
        self.camera.yaw = (a[1] - 90.0).to_radians();
        self.camera.roll = a[2].to_radians();
        self.camera.fov_y = a[3].to_radians();
        self.path_tracer.reset();
        format!("camera {what} {value}")
    }
}
