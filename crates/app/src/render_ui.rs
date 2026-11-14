//! The Render panel: sun, sky, ground and film, edited from the interface.
//!
//! The settings live in the document as the file's render objects. The
//! panel edits a copy of the decoded `Environment`, writes it back into
//! objects and records that as one `Command::SetRender`, so a render change
//! is undoable, marks the file as changed and is saved with it. Everything
//! else (the tracer, the sky bake) follows from the objects.

use crate::App;
use crate::pathtrace::{Background, Environment, LensProjection, VoxelShape, linear_to_srgb, srgb_to_linear};
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

/// What a material copy leaves in the system clipboard, so its Paste is
/// recognised as one.
const MATERIAL_MARKER: &str = "Voxferro material";

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
        // The render camera's field of view is a lens setting. A wide lens
        // may ask for up to a full turn; the perspective stops short of
        // half of one.
        self.camera.fov_y = self.environment.lens_fov.min(178.0).to_radians();
        if let Some(message) = self.ensure_env_map(state)
            && message.contains("not found")
        {
            self.console_log.push(message);
        }
        self.path_tracer.reset();
    }

    /// A click on the render: the depth there becomes the focus, fixed, and
    /// depth of field turns on ("Click Voxel to Set Focus").
    /// The depth is along the view in perspective, along the ray through
    /// the wide lenses, as each focuses; the nearest visible voxel or the
    /// ground.
    pub fn focus_at(&mut self, pos: egui::Pos2, rect: egui::Rect) -> String {
        if self.camera.ortho() {
            return "a parallel view has no focus".to_owned();
        }
        let Some(loaded) = &self.scene else { return "nothing open".to_owned() };
        let shown = match self.photo_size() {
            Some([w, h]) => crate::photo::letterbox(rect, w as f32 / h as f32),
            None => rect,
        };
        let aspect = shown.width() / shown.height().max(1.0);
        let st = glam::Vec2::new(
            ((pos.x - shown.left()) / shown.width() * 2.0 - 1.0) * aspect,
            1.0 - (pos.y - shown.top()) / shown.height() * 2.0,
        );
        let env = &self.environment;
        let factor = match env.projection {
            LensProjection::Perspective => env.projection.view_factor(self.camera.fov_y.to_degrees()),
            wide => wide.view_factor(env.lens_fov),
        };
        let basis = self.camera.basis();
        let direction = (basis * env.projection.direction(st, factor)).normalize();
        let eye = self.camera.eye();
        let visible = self.visibility();
        let voxel = loaded.doc.scene.pick(eye, direction, crate::edit::PICK_RANGE, |i| visible.shows(i)).map(|p| p.distance);
        let ground = (env.ground_enabled && direction.z < 0.0 && eye.z > 0.0).then(|| -eye.z / direction.z);
        let Some(distance) = [voxel, ground].into_iter().flatten().reduce(f32::min) else {
            return "nothing there to focus on".to_owned();
        };
        let depth = match env.projection {
            LensProjection::Perspective => distance * direction.dot(basis.z_axis),
            _ => distance,
        };
        self.focus = depth;
        self.fix_focus = true;
        self.dof = true;
        self.environment.dof = true;
        self.path_tracer.reset();
        format!("focus at {depth:.1}")
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

        // The background and voxel shapes: settings of the session.
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
            // Lines on the faces: cubes only.
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
                changed |= ui
                    .add(egui::Slider::new(&mut lines.edge_width, 0.01..=crate::pathtrace::EDGE_WIDTH_MAX).text("width"))
                    .changed();
            }
            if changed {
                self.apply_look();
                self.path_tracer.reset();
            }
        });

        egui::CollapsingHeader::new("Lens").show(ui, |ui| {
            ui.horizontal(|ui| {
                for (projection, name, hint) in [
                    (LensProjection::Perspective, "Perspective", "Perspective projection"),
                    (LensProjection::Stereographic, "SG", "Stereographic projection: wide angles without stretching at the edges"),
                    (LensProjection::Panorama, "Panorama", "Every direction around the camera; a 2:1 image holds the whole turn"),
                ] {
                    clicked |= ui.selectable_value(&mut env.projection, projection, name).on_hover_text(hint).changed();
                }
            });
            dragged |= ui.add(egui::Slider::new(&mut env.lens_fov, 1.0..=360.0).text("fov"))
                .on_hover_text("Perspective reaches 178 degrees, stereographic 356; the panorama has its own")
                .changed();
            // Depth of field is a switch of the session: the file has no key
            // for it, as in the editor.
            if ui
                .checkbox(&mut self.dof, "depth of field")
                .on_hover_text("Click the render to focus there; until then, the point the camera orbits is in focus")
                .changed()
            {
                self.environment.dof = self.dof;
                self.path_tracer.reset();
            }
            ui.horizontal(|ui| {
                let mut changed = ui
                    .checkbox(&mut self.fix_focus, "fixed focus")
                    .on_hover_text("Keep the focus depth when the camera moves; a click in the render sets it")
                    .changed();
                changed |= ui
                    .add_enabled(self.fix_focus, egui::DragValue::new(&mut self.focus).range(0.0..=9999.9).speed(0.5).max_decimals(1))
                    .changed();
                if changed {
                    self.path_tracer.reset();
                }
            });
            dragged |= ui.add(egui::Slider::new(&mut env.aperture, 0.0..=2.0).text("aperture")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.blades, 0..=12).text("blades")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.blade_rotation, 0.0..=360.0).text("blade angle")).changed();
        });

        egui::CollapsingHeader::new("Film").show(ui, |ui| {
            dragged |= ui.add(egui::Slider::new(&mut env.exposure, 0.05..=5.0).text("exposure")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.vignette, 0.0..=1.0).text("vignette")).changed();
            dragged |= ui.add(egui::Slider::new(&mut env.gamma, 1.0..=3.0).text("gamma")).changed();
            clicked |= ui.checkbox(&mut env.aces, "ACES tone curve").changed();
        });

        // Render › Sample, with its ranges.
        egui::CollapsingHeader::new("Sample").show(ui, |ui| {
            dragged |= ui.add(egui::Slider::new(&mut env.bounces, 0..=15).text("diffuse"))
                .on_hover_text("Diffuse bounces after the first hit; 0 is direct light only").changed();
            dragged |= ui.add(egui::Slider::new(&mut env.specular_bounces, 5..=15).text("specular"))
                .on_hover_text("Reflections and refractions a path may take").changed();
            dragged |= ui.add(egui::Slider::new(&mut env.scatter_bounces, 5..=15).text("scatter"))
                .on_hover_text("Scattering in clouds and fog").changed();
            dragged |= ui.add(egui::Slider::new(&mut env.energy, 1.0..=5000.0).logarithmic(true).text("energy"))
                .on_hover_text("The most light one sample may carry: lower removes fireflies").changed();
            if ui.checkbox(&mut self.look.antialias, "anti-aliasing").on_hover_text("Off, every sample goes through the pixel's centre").changed() {
                self.apply_look();
                self.path_tracer.reset();
            }
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
            Some("ground") => {
                env.ground_enabled = match args.get(1) {
                    Some(&"on") => true,
                    Some(&"off") => false,
                    None => !env.ground_enabled,
                    _ => return "env ground [on|off]".to_owned(),
                };
                format!("ground {}", if env.ground_enabled { "on" } else { "off" })
            }
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
            Some("bounce") => {
                // The ranges: diffuse 0-15, specular and scatter 5-15.
                let n = |i: usize, lo: u32| args.get(i).and_then(|v| v.parse::<u32>().ok()).map(|v| v.clamp(lo, 15));
                let (Some(diffuse), specular, scatter) = (n(1, 0), n(2, 5), n(3, 5)) else {
                    return "env bounce <diffuse 0-15> [specular 5-15] [scatter 5-15]".to_owned();
                };
                env.bounces = diffuse;
                env.specular_bounces = specular.unwrap_or(env.specular_bounces);
                env.scatter_bounces = scatter.unwrap_or(env.scatter_bounces);
                format!("bounces: diffuse {} specular {} scatter {}", env.bounces, env.specular_bounces, env.scatter_bounces)
            }
            Some("energy") => {
                let Some(e) = args.get(1).and_then(|v| v.parse::<f32>().ok()) else {
                    return "env energy <1-5000>".to_owned();
                };
                env.energy = e.clamp(1.0, 5000.0);
                format!("energy limit {}", env.energy)
            }
            Some("aa") => {
                self.look.antialias = match args.get(1) {
                    Some(&"on") => true,
                    Some(&"off") => false,
                    None => !self.look.antialias,
                    _ => return "env aa [on|off]".to_owned(),
                };
                self.apply_look();
                self.path_tracer.reset();
                return format!("anti-aliasing {}", if self.look.antialias { "on" } else { "off" });
            }
            Some("proj") => {
                env.projection = match args.get(1).copied() {
                    Some("perspective" | "pers") => LensProjection::Perspective,
                    Some("sg" | "stereographic") => LensProjection::Stereographic,
                    Some("panorama" | "pano") => LensProjection::Panorama,
                    _ => return "env proj perspective|sg|panorama".to_owned(),
                };
                if let Some(fov) = args.get(2).and_then(|v| v.parse::<f32>().ok()) {
                    env.lens_fov = fov.clamp(1.0, 360.0);
                }
                format!("lens: {:?}, fov {}", env.projection, env.lens_fov).to_lowercase()
            }
            Some("focus") => {
                match args.get(1).copied() {
                    Some("auto" | "off") => self.fix_focus = false,
                    Some(v) => match v.parse::<f32>() {
                        Ok(d) => {
                            self.focus = d.clamp(0.0, 9999.9);
                            self.fix_focus = true;
                        }
                        Err(_) => return "env focus <depth> | auto".to_owned(),
                    },
                    None => return "env focus <depth> | auto".to_owned(),
                }
                self.path_tracer.reset();
                return if self.fix_focus { format!("focus fixed at {}", self.focus) } else { "focus on the orbit point".to_owned() };
            }
            Some("vig") => {
                env.vignette = args.get(1).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0).clamp(0.0, 1.0);
                format!("vignette {}", env.vignette)
            }
            _ => {
                return "env uniform | atmosphere [elev] | ibl [map] [i] [rot] | dof [on|off] | background sky|none|<colour> | shape <kind> [size] | grid|edge on|off [width] | \
                        lens [aperture] [blades] [angle] | bloom [mix] [scale] [aspect] [threshold] | vig <v> | fog <density> [g] | \
                        bounce <diffuse> [specular] [scatter] | energy <n> | aa [on|off] | proj perspective|sg|panorama [fov] | focus <depth>|auto | ground [on|off]"
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
    /// Render › Sample › Anti-Aliasing.
    pub antialias: bool,
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
            antialias: true,
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
            let max = if grid { 1.0 } else { crate::pathtrace::EDGE_WIDTH_MAX };
            let width = rest.first().and_then(|v| v.parse::<f32>().ok()).map(|w| w.clamp(0.01, max));
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

    /// The render's background between the sky and its colour (Cmd+B in
    /// the render).
    pub fn render_background_toggle(&mut self) -> String {
        self.look.background = match self.look.background {
            Background::Colour => Background::Sky,
            _ => Background::Colour,
        };
        self.apply_look();
        self.path_tracer.reset();
        format!("background {:?}", self.look.background).to_lowercase()
    }

    /// The renderer's own keys: Cmd+U the ground, Cmd+C and
    /// Cmd+V the brush colour's material, Cmd+R that material back to
    /// diffuse. (Cmd+E, G and B are with the display switches.)
    pub fn render_input(&mut self, ctx: &egui::Context, state: &egui_wgpu::RenderState) {
        if !self.rendering || ctx.egui_wants_keyboard_input() {
            return;
        }
        let mut lines: Vec<&str> = Vec::new();
        ctx.input(|i| {
            let cmd = i.modifiers.command && !i.modifiers.shift;
            if cmd && i.key_pressed(egui::Key::U) {
                lines.push("env ground");
            }
            if cmd && i.key_pressed(egui::Key::R) {
                lines.push("material reset");
            }
            for event in &i.events {
                match event {
                    egui::Event::Copy => lines.push("material copy"),
                    egui::Event::Paste(text) if text == MATERIAL_MARKER => lines.push("material paste"),
                    _ => {}
                }
            }
        });
        for line in lines {
            let message = if line.starts_with("env") { self.env_command(line, state) } else { self.palette_command(line, state) };
            if line == "material copy" {
                ctx.copy_text(MATERIAL_MARKER.to_owned());
            }
            self.console_log.push(message);
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
        e.antialias = self.look.antialias;
        let rgb = |c: [u8; 3]| glam::Vec3::new(lin(c[0]), lin(c[1]), lin(c[2]));
        e.lines = crate::pathtrace::Lines {
            grid_color: rgb(self.look.grid_rgb),
            edge_color: rgb(self.look.edge_rgb),
            ..self.look.lines
        };
    }
}
