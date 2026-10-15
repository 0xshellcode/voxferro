//! The Render panel: sun, sky, ground and film, edited from the interface.
//!
//! The settings live in the document as the file's render objects. The
//! panel edits a copy of the decoded `Environment`, writes it back into
//! objects and records that as one `Command::SetRender`, so a render change
//! is undoable, marks the file as changed and is saved with it. Everything
//! else (the tracer, the sky bake) follows from the objects.

use crate::App;
use crate::pathtrace::{Environment, linear_to_srgb, srgb_to_linear};
use eframe::{egui, egui_wgpu};
use glam::Vec3;
use voxel_core::Command;

/// The sky panel's three modes, as `_env._mode` numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sky {
    Uniform,
    Atmosphere,
    Image,
}

impl App {
    /// Records `env` as the document's render settings. `merge` folds the
    /// frames of one slider drag into a single undo step.
    pub fn apply_environment(&mut self, env: &Environment, merge: bool, state: &egui_wgpu::RenderState) {
        let Some(loaded) = self.scene.as_mut() else { return };
        let after = env.write_render_objects(&loaded.doc.render);
        let command = Command::SetRender { before: loaded.doc.render.clone(), after };
        let changed = if merge {
            self.history.exec(&mut loaded.doc, command)
        } else {
            self.history.exec_alone(&mut loaded.doc, command)
        };
        if changed {
            self.sync_environment(state);
        }
    }

    /// Decodes the document's render objects into what the tracer uses,
    /// after an edit, an undo or a load.
    pub fn sync_environment(&mut self, state: &egui_wgpu::RenderState) {
        let Some(loaded) = &self.scene else { return };
        self.environment = Environment::from_render_objects(&loaded.doc.render);
        self.environment_source = loaded.doc.render.clone();
        if let Some(message) = self.ensure_env_map(state) {
            if message.contains("not found") {
                self.console_log.push(message);
            }
        }
        self.path_tracer.reset();
    }

    /// Environment maps found in the search folders, by file name.
    fn available_maps(&mut self) -> &[String] {
        if self.env_maps.is_none() {
            let mut names: Vec<String> = crate::env_map_dirs()
                .into_iter()
                .filter_map(|d| std::fs::read_dir(d).ok())
                .flat_map(|entries| entries.flatten())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.to_ascii_lowercase().ends_with(".hdr"))
                .collect();
            names.sort();
            names.dedup();
            self.env_maps = Some(names);
        }
        self.env_maps.as_deref().unwrap_or_default()
    }

    pub fn render_settings_ui(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        if self.scene.is_none() {
            return;
        }
        let mut env = self.environment.clone();
        // A slider reports `changed` every frame it moves; those merge. A
        // click (mode, checkbox, map) is a step of its own.
        let mut dragged = false;
        let mut clicked = false;

        egui::CollapsingHeader::new("Sun").default_open(true).show(ui, |ui| {
            let d = env.sun_direction.normalize_or(Vec3::Z);
            let mut azimuth = (-d.y).atan2(d.x).to_degrees().rem_euclid(360.0);
            let mut elevation = d.z.clamp(-1.0, 1.0).asin().to_degrees();
            let a = ui.add(egui::Slider::new(&mut azimuth, 0.0..=360.0).text("azimuth"));
            let e = ui.add(egui::Slider::new(&mut elevation, -10.0..=90.0).text("elevation"));
            if a.changed() || e.changed() {
                let (az, el) = (azimuth.to_radians(), elevation.to_radians());
                env.sun_direction = Vec3::new(el.cos() * az.cos(), -el.cos() * az.sin(), el.sin());
                dragged = true;
            }
            dragged |= ui.add(egui::Slider::new(&mut env.sun_intensity, 0.0..=10.0).text("intensity")).changed();
            dragged |= ui
                .add(egui::Slider::new(&mut env.sun_angular_radius, 0.01..=0.5).text("softness"))
                .changed();
            ui.horizontal(|ui| {
                ui.label("colour");
                dragged |= stored_colour_edit(ui, &mut env.sun_color);
            });
        });

        egui::CollapsingHeader::new("Sky").default_open(true).show(ui, |ui| {
            let mut sky = if env.ibl_enabled {
                Sky::Image
            } else if env.atmosphere_enabled {
                Sky::Atmosphere
            } else {
                Sky::Uniform
            };
            ui.horizontal(|ui| {
                for (mode, name) in [(Sky::Uniform, "Uniform"), (Sky::Atmosphere, "Atmosphere"), (Sky::Image, "Image")] {
                    if ui.selectable_value(&mut sky, mode, name).clicked() {
                        env.ibl_enabled = sky == Sky::Image;
                        env.atmosphere_enabled = sky == Sky::Atmosphere;
                        clicked = true;
                    }
                }
            });
            match sky {
                Sky::Uniform => {
                    dragged |= ui.add(egui::Slider::new(&mut env.sky_intensity, 0.0..=4.0).text("intensity")).changed();
                    ui.horizontal(|ui| {
                        ui.label("colour");
                        dragged |= stored_colour_edit(ui, &mut env.sky_color);
                    });
                }
                Sky::Atmosphere => {
                    let a = &mut env.atmosphere;
                    dragged |= ui.add(egui::Slider::new(&mut a.ray_density, 0.0..=2.0).text("rayleigh")).changed();
                    dragged |= ui.add(egui::Slider::new(&mut a.mie_density, 0.0..=2.0).text("mie")).changed();
                    dragged |= ui.add(egui::Slider::new(&mut a.mie_g, 0.0..=0.99).text("mie g")).changed();
                    dragged |= ui.add(egui::Slider::new(&mut a.ozone_density, 0.0..=2.0).text("ozone")).changed();
                }
                Sky::Image => {
                    let current = env.ibl_path.clone();
                    let maps = self.available_maps().to_vec();
                    egui::ComboBox::from_id_salt("environment map")
                        .selected_text(current.as_str())
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            for name in maps {
                                if ui.selectable_label(name == current, &name).clicked() {
                                    env.ibl_path = name;
                                    clicked = true;
                                }
                            }
                        });
                    dragged |= ui.add(egui::Slider::new(&mut env.ibl_intensity, 0.0..=4.0).text("intensity")).changed();
                    dragged |= ui.add(egui::Slider::new(&mut env.ibl_rotation, 0.0..=1.0).text("rotation")).changed();
                }
            }
        });

        egui::CollapsingHeader::new("Ground").show(ui, |ui| {
            clicked |= ui.checkbox(&mut env.ground_enabled, "ground plane").changed();
            dragged |= ui.add(egui::Slider::new(&mut env.horizon_density, 0.0..=1.0).text("horizon")).changed();
            ui.horizontal(|ui| {
                ui.label("colour");
                // Decoded to linear on reading; edited as the sRGB bytes the
                // file holds.
                let g = env.ground_color;
                let mut rgb = [g.x, g.y, g.z].map(linear_to_srgb);
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    env.ground_color = Vec3::from_array(rgb.map(srgb_to_linear));
                    dragged = true;
                }
            });
        });

        egui::CollapsingHeader::new("Film").show(ui, |ui| {
            dragged |= ui.add(egui::Slider::new(&mut env.exposure, 0.05..=5.0).text("exposure")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.bounces, 1..=8).text("bounces")).changed();
        });

        if dragged || clicked {
            self.apply_environment(&env, dragged && !clicked, state);
        }
    }

    /// `env uniform` | `env atmosphere [sun elevation]` |
    /// `env ibl [map.hdr] [intensity] [rotation]`
    pub fn env_command(&mut self, line: &str, state: &egui_wgpu::RenderState) -> String {
        let args: Vec<&str> = line.split_whitespace().skip(1).collect();
        let mut env = self.environment.clone();
        let message = match args.first().copied() {
            Some("uniform") => {
                env.ibl_enabled = false;
                env.atmosphere_enabled = false;
                "sky: uniform".to_owned()
            }
            Some("atmosphere") => {
                env.ibl_enabled = false;
                env.atmosphere_enabled = true;
                // Optional sun elevation, degrees, to try sunsets.
                if let Some(el) = args.get(1).and_then(|v| v.parse::<f32>().ok()) {
                    let d = env.sun_direction;
                    let az = (-d.y).atan2(d.x);
                    let el = el.to_radians();
                    env.sun_direction = Vec3::new(el.cos() * az.cos(), -el.cos() * az.sin(), el.sin());
                }
                "sky: atmosphere".to_owned()
            }
            Some("ibl") => {
                if let Some(name) = args.get(1) {
                    env.ibl_path = (*name).to_owned();
                }
                if let Some(i) = args.get(2).and_then(|v| v.parse::<f32>().ok()) {
                    env.ibl_intensity = i.max(0.0);
                }
                if let Some(r) = args.get(3).and_then(|v| v.parse::<f32>().ok()) {
                    env.ibl_rotation = r;
                }
                env.ibl_enabled = true;
                env.atmosphere_enabled = false;
                format!("sky: {}", env.ibl_path)
            }
            _ => return "env uniform | env atmosphere [sun elevation] | env ibl [map.hdr] [i] [rot]".to_owned(),
        };
        self.apply_environment(&env, false, state);
        // A missing map falls back to the uniform sky; say so.
        if env.ibl_enabled && !self.environment.ibl_enabled {
            return format!("environment map {} not found; using the uniform sky", env.ibl_path);
        }
        message
    }
}

/// A colour kept as the file's bytes over 255 (the sun's and sky's `_k`),
/// edited as those same sRGB bytes. egui's float picker would take them for
/// linear values and show them too light.
fn stored_colour_edit(ui: &mut egui::Ui, colour: &mut Vec3) -> bool {
    let mut rgb = [colour.x, colour.y, colour.z].map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8);
    let changed = ui.color_edit_button_srgb(&mut rgb).changed();
    if changed {
        *colour = Vec3::from_array(rgb.map(|c| c as f32 / 255.0));
    }
    changed
}
