//! The Render panel: sun, sky, ground and film, edited from the interface.
//!
//! The settings live in the document as the file's render objects. The
//! panel edits a copy of the decoded `Environment`, writes it back into
//! objects and records that as one `Command::SetRender`, so a render change
//! is undoable, marks the file as changed and is saved with it. Everything
//! else (the tracer, the sky bake) follows from the objects.

use crate::App;
use crate::pathtrace::{Background, Environment, VoxelShape, linear_to_srgb, srgb_to_linear};
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
        // Not in the file: switches of this session.
        self.environment.dof = self.dof;
        self.apply_look();
        // The render camera's field of view is a lens setting.
        self.camera.fov_y = self.environment.lens_fov.to_radians();
        if let Some(message) = self.ensure_env_map(state)
            && message.contains("not found")
        {
            self.console_log.push(message);
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

        egui::CollapsingHeader::new("Fog").show(ui, |ui| {
            dragged |= ui.add(egui::Slider::new(&mut env.fog_density, 0.0..=20.0).text("density")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.fog_g, -0.9..=0.9).text("phase g")).changed();
            ui.horizontal(|ui| {
                ui.label("colour");
                dragged |= stored_colour_edit(ui, &mut env.fog_color);
            });
        });

        // the reference editor's background and voxel shapes (`pt_b_bg_a`,
        // `pt_bg_col`, `pt_prim`, `pt_cell`): settings of the session.
        egui::CollapsingHeader::new("Look").show(ui, |ui| {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label("Background");
                for (b, name) in [(Background::Sky, "sky"), (Background::Colour, "colour"), (Background::Transparent, "none")] {
                    changed |= ui.selectable_value(&mut self.look.background, b, name).changed();
                }
            });
            if self.look.background == Background::Colour {
                changed |= egui::color_picker::color_edit_button_srgb(ui, &mut self.look.background_rgb).changed();
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Voxels");
                for (shape, name) in VoxelShape::ALL {
                    changed |= ui.selectable_value(&mut self.look.shape, shape, name).changed();
                }
            });
            if matches!(self.look.shape, VoxelShape::Sphere | VoxelShape::Cylinder) {
                changed |= ui.add(egui::Slider::new(&mut self.look.size, 0.0..=1.0).text("size")).changed();
            }
            // Lines on the faces (`pt_b_grid`, `pt_b_edge`): cubes only.
            let lines = &mut self.look.lines;
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut lines.grid, "Grid").changed();
                changed |= egui::color_picker::color_edit_button_srgb(ui, &mut self.look.grid_rgb).changed();
                changed |= ui.checkbox(&mut lines.grid_ground_only, "ground only").changed();
            });
            if lines.grid {
                changed |= ui.add(egui::Slider::new(&mut lines.grid_spacing, 1..=64).text("spacing")).changed();
                changed |= ui.add(egui::Slider::new(&mut lines.grid_width, 0.01..=1.0).text("width")).changed();
            }
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut lines.edge, "Edges").changed();
                changed |= egui::color_picker::color_edit_button_srgb(ui, &mut self.look.edge_rgb).changed();
            });
            if lines.edge {
                changed |= ui.add(egui::Slider::new(&mut lines.edge_width, 0.01..=1.0).text("width")).changed();
            }
            if changed {
                self.apply_look();
                self.path_tracer.reset();
            }
        });

        egui::CollapsingHeader::new("Lens").show(ui, |ui| {
            dragged |= ui.add(egui::Slider::new(&mut env.lens_fov, 5.0..=120.0).text("fov")).changed();
            // Depth of field is a switch of the session: the file has no key
            // for it, as in the editor. It focuses on the orbit target.
            if ui
                .checkbox(&mut self.dof, "depth of field")
                .on_hover_text("Focus is the point the camera orbits: frame or pan to move it")
                .changed()
            {
                self.environment.dof = self.dof;
                self.path_tracer.reset();
            }
            dragged |= ui.add(egui::Slider::new(&mut env.aperture, 0.0..=2.0).text("aperture")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.blades, 0..=12).text("blades")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.blade_rotation, 0.0..=360.0).text("blade angle")).changed();
        });

        egui::CollapsingHeader::new("Film").show(ui, |ui| {
            dragged |= ui.add(egui::Slider::new(&mut env.exposure, 0.05..=5.0).text("exposure")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.vignette, 0.0..=1.0).text("vignette")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.gamma, 1.0..=3.0).text("gamma")).changed();
            clicked |= ui.checkbox(&mut env.aces, "ACES tone curve").changed();
            dragged |= ui.add(egui::Slider::new(&mut env.bounces, 1..=8).text("bounces")).changed();
        });

        egui::CollapsingHeader::new("Bloom").show(ui, |ui| {
            dragged |= ui.add(egui::Slider::new(&mut env.bloom_mix, 0.0..=2.0).text("mix")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.bloom_scale, 0.0..=1.0).text("scale")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.bloom_aspect, -1.0..=1.0).text("aspect")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.bloom_threshold, 0.0..=4.0).text("threshold")).changed();
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
            Some("background") => {
                match args.get(1..).unwrap_or_default() {
                    ["sky"] => self.look.background = Background::Sky,
                    ["none" | "transparent"] => self.look.background = Background::Transparent,
                    rest => match crate::palette_ops::parse_colour(rest) {
                        Some(c) => {
                            self.look.background = Background::Colour;
                            self.look.background_rgb = [c.r, c.g, c.b];
                        }
                        None => return "env background sky | none | #rrggbb | r g b".to_owned(),
                    },
                }
                self.apply_look();
                self.path_tracer.reset();
                return format!("background {:?}", self.look.background).to_lowercase();
            }
            Some("shape") => {
                let Some(shape) = args.get(1).and_then(|w| VoxelShape::ALL.iter().find(|(_, n)| n == w)).map(|(s, _)| *s) else {
                    return "env shape cube|sphere|cylinder|brick|clay|mc [size 0-1]".to_owned();
                };
                self.look.shape = shape;
                if let Some(size) = args.get(2).and_then(|v| v.parse::<f32>().ok()) {
                    self.look.size = size.clamp(0.0, 1.0);
                }
                self.apply_look();
                self.path_tracer.reset();
                return format!("voxels as {}", args[1]);
            }
            Some("grid" | "edge") => return self.lines_command(&args),
            Some("dof") => {
                self.dof = args.get(1).is_none_or(|v| *v != "off");
                self.environment.dof = self.dof;
                self.path_tracer.reset();
                return format!("depth of field {}", if self.dof { "on" } else { "off" });
            }
            Some("lens") => {
                let n = |i: usize| args.get(i).and_then(|v| v.parse::<f32>().ok());
                if let Some(v) = n(1) {
                    env.aperture = v.max(0.0);
                }
                if let Some(v) = n(2) {
                    env.blades = v.max(0.0) as u32;
                }
                if let Some(v) = n(3) {
                    env.blade_rotation = v;
                }
                format!("lens: aperture {} blades {} angle {}", env.aperture, env.blades, env.blade_rotation)
            }
            Some("bloom") => {
                let n = |i: usize| args.get(i).and_then(|v| v.parse::<f32>().ok());
                if let Some(v) = n(1) {
                    env.bloom_mix = v.max(0.0);
                }
                if let Some(v) = n(2) {
                    env.bloom_scale = v.clamp(0.0, 1.0);
                }
                if let Some(v) = n(3) {
                    env.bloom_aspect = v.clamp(-1.0, 1.0);
                }
                if let Some(v) = n(4) {
                    env.bloom_threshold = v.max(0.0);
                }
                format!(
                    "bloom: mix {} scale {} aspect {} threshold {}",
                    env.bloom_mix, env.bloom_scale, env.bloom_aspect, env.bloom_threshold
                )
            }
            Some("fog") => {
                if let Some(d) = args.get(1).and_then(|v| v.parse::<f32>().ok()) {
                    env.fog_density = d.max(0.0);
                }
                if let Some(g) = args.get(2).and_then(|v| v.parse::<f32>().ok()) {
                    env.fog_g = g.clamp(-0.99, 0.99);
                }
                format!("fog: density {} g {}", env.fog_density, env.fog_g)
            }
            Some("vig") => {
                env.vignette = args.get(1).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0).clamp(0.0, 1.0);
                format!("vignette {}", env.vignette)
            }
            _ => {
                return "env uniform | atmosphere [elev] | ibl [map] [i] [rot] | dof [on|off] | background sky|none|<colour> | shape <kind> [size] | grid|edge on|off [width] | \
                        lens [aperture] [blades] [angle] | bloom [mix] [scale] [aspect] [threshold] | vig <v> | fog <density> [g]"
                    .to_owned();
            }
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

/// The render's look for the session: background, voxel shape, lines.
pub struct Look {
    pub background: Background,
    pub background_rgb: [u8; 3],
    pub shape: VoxelShape,
    pub size: f32,
    /// Colours kept here as sRGB bytes, handed over linear.
    pub lines: crate::pathtrace::Lines,
    pub grid_rgb: [u8; 3],
    pub edge_rgb: [u8; 3],
}

impl Default for Look {
    fn default() -> Self {
        Self {
            background: Background::Sky,
            background_rgb: [50, 50, 50],
            shape: VoxelShape::Cube,
            size: 1.0,
            lines: Default::default(),
            grid_rgb: [0, 0, 0],
            edge_rgb: [0, 0, 0],
        }
    }
}

impl App {
    /// `env grid …` and `env edge …`: the render's lines, `args` from the
    /// word `grid` or `edge` on.
    pub fn lines_command(&mut self, args: &[&str]) -> String {
        let grid = args[0] == "grid";
        let usage = "env grid on|off [width] [spacing] [ground] | env edge on|off [width] | env grid|edge colour <colour>";
        let lines = &mut self.look.lines;
        if args.get(1).is_some_and(|w| matches!(*w, "colour" | "color")) {
            let Some(c) = crate::palette_ops::parse_colour(&args[2..]) else { return usage.to_owned() };
            *(if grid { &mut self.look.grid_rgb } else { &mut self.look.edge_rgb }) = [c.r, c.g, c.b];
        } else {
            let rest = args.get(2..).unwrap_or_default();
            let on = match args.get(1) {
                Some(&"on") => true,
                Some(&"off") => false,
                None => !(if grid { lines.grid } else { lines.edge }),
                _ => return usage.to_owned(),
            };
            let width = rest.first().and_then(|v| v.parse::<f32>().ok()).map(|w| w.clamp(0.01, 1.0));
            if grid {
                lines.grid = on;
                lines.grid_width = width.unwrap_or(lines.grid_width);
                if let Some(s) = rest.get(1).and_then(|v| v.parse::<u32>().ok()) {
                    lines.grid_spacing = s.max(1);
                }
                lines.grid_ground_only = rest.get(2).is_some_and(|w| *w == "ground");
            } else {
                lines.edge = on;
                lines.edge_width = width.unwrap_or(lines.edge_width);
            }
        }
        self.apply_look();
        self.path_tracer.reset();
        let l = &self.look.lines;
        if grid {
            format!("grid {} (width {}, every {})", if l.grid { "on" } else { "off" }, l.grid_width, l.grid_spacing)
        } else {
            format!("edges {} (width {})", if l.edge { "on" } else { "off" }, l.edge_width)
        }
    }

    /// Hands the session's look to the tracer's settings.
    pub fn apply_look(&mut self) {
        let e = &mut self.environment;
        e.background = self.look.background;
        let lin = |v: u8| (v as f32 / 255.0).powf(2.2);
        e.background_color = glam::Vec3::new(lin(self.look.background_rgb[0]), lin(self.look.background_rgb[1]), lin(self.look.background_rgb[2]));
        e.voxel_shape = self.look.shape;
        e.voxel_size = self.look.size;
        let rgb = |c: [u8; 3]| glam::Vec3::new(lin(c[0]), lin(c[1]), lin(c[2]));
        e.lines = crate::pathtrace::Lines {
            grid_color: rgb(self.look.grid_rgb),
            edge_color: rgb(self.look.edge_rgb),
            ..self.look.lines
        };
    }
}
