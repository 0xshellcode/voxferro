//! Editing palette colours and materials.
//!
//! Both act on the brush's palette index, as the editor's palette panel does.
//! Every change is a command, so a colour or a material is undone like any
//! stroke; dragging a slider merges into one step until the button is
//! released.

use crate::App;
use eframe::{egui, egui_wgpu};
use vox_format::{Color, Material, MaterialKind, MediaKind, ResolvedMaterial};
use voxel_core::Command;

const KINDS: [(MaterialKind, &str); 5] = [
    (MaterialKind::Diffuse, "diffuse"),
    (MaterialKind::Metal, "metal"),
    (MaterialKind::Glass, "glass"),
    (MaterialKind::Emissive, "emit"),
    (MaterialKind::Media, "cloud"),
];

const MEDIA: [(MediaKind, &str); 4] = [
    (MediaKind::Absorb, "absorb"),
    (MediaKind::Scatter, "scatter"),
    (MediaKind::Emit, "emit"),
    (MediaKind::Subsurface, "sss"),
];

/// What a freshly chosen kind starts with, as the editor's are fully on.
fn start_kind(r: &mut ResolvedMaterial, kind: MaterialKind) {
    match kind {
        MaterialKind::Metal => r.metalness = 1.0,
        MaterialKind::Glass => r.transparency = 1.0,
        MaterialKind::Emissive => r.emission = 1.0,
        MaterialKind::Media => {
            r.media = MediaKind::Scatter;
            if r.density <= 0.0 {
                r.density = 0.5;
            }
        }
        MaterialKind::Diffuse => {}
    }
}

impl App {
    pub fn palette_editor_ui(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        let Some(loaded) = &self.scene else { return };
        let index = self.brush_color;
        let colour = loaded.doc.palette.color(index);
        let material = loaded.doc.material(index).cloned();
        let resolved = material.as_ref().map(Material::resolve).unwrap_or_default();

        ui.horizontal(|ui| {
            ui.label(format!("Index {index}"));
            let mut rgb = [colour.r, colour.g, colour.b];
            if egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                self.set_colour(index, Color { r: rgb[0], g: rgb[1], b: rgb[2], a: 255 }, state);
            }
            ui.label(format!("#{:02x}{:02x}{:02x}", colour.r, colour.g, colour.b));
        });

        let mut r = resolved;
        let mut changed = false;
        ui.horizontal(|ui| {
            for (kind, name) in KINDS {
                if ui.selectable_label(r.kind == kind, name).clicked() && r.kind != kind {
                    r.kind = kind;
                    start_kind(&mut r, kind);
                    changed = true;
                }
            }
        });
        let mut media_changed = false;
        let mut slider = |ui: &mut egui::Ui, v: &mut f32, range: std::ops::RangeInclusive<f32>, label: &str| {
            changed |= ui.add(egui::Slider::new(v, range).text(label)).changed();
        };
        match r.kind {
            MaterialKind::Diffuse => {}
            MaterialKind::Metal => {
                slider(ui, &mut r.metalness, 0.0..=1.0, "metal");
                slider(ui, &mut r.roughness, 0.0..=1.0, "rough");
            }
            MaterialKind::Glass => {
                slider(ui, &mut r.transparency, 0.0..=1.0, "trans");
                slider(ui, &mut r.roughness, 0.0..=1.0, "rough");
                slider(ui, &mut r.ior, 1.0..=3.0, "ior");
                slider(ui, &mut r.density, 0.0..=1.0, "density");
            }
            MaterialKind::Emissive => {
                slider(ui, &mut r.emission, 0.0..=1.0, "emit");
                slider(ui, &mut r.flux, 0.0..=4.0, "flux");
            }
            MaterialKind::Media => {
                ui.horizontal(|ui| {
                    for (media, name) in MEDIA {
                        if ui.selectable_label(r.media == media, name).clicked() && r.media != media {
                            r.media = media;
                            media_changed = true;
                        }
                    }
                });
                slider(ui, &mut r.density, 0.0..=4.0, "density");
                slider(ui, &mut r.phase, -0.9..=0.9, "phase g");
            }
        }
        changed |= media_changed;
        if changed {
            self.set_material(index, Some(r), state);
        }
        if material.is_some() && ui.small_button("Reset material").clicked() {
            self.set_material(index, None, state);
        }
    }

    fn set_colour(&mut self, index: u8, colour: Color, state: &egui_wgpu::RenderState) {
        let Some(loaded) = self.scene.as_mut() else { return };
        let before = loaded.doc.palette.color(index);
        if self.history.exec(&mut loaded.doc, Command::SetColor { index, before, after: colour }) {
            self.refresh_colors(state);
        }
    }

    /// `None` removes the material: plain diffuse.
    fn set_material(&mut self, index: u8, edited: Option<ResolvedMaterial>, state: &egui_wgpu::RenderState) {
        let Some(loaded) = self.scene.as_mut() else { return };
        let before = loaded.doc.material(index).cloned();
        let after = edited.map(|r| Material::from_resolved(index as i32, before.as_ref(), &r));
        if self.history.exec(&mut loaded.doc, Command::SetMaterial { index, before, after }) {
            self.refresh_colors(state);
            // Emitters are found by material, so a colour turned emissive
            // (or back) changes what the tracer samples.
            self.upload_trace_volume(state);
        }
    }

    /// `palette <index> <r> <g> <b>` and
    /// `material <index> <diffuse|metal|glass|emit|cloud> [key=value]...`,
    /// where keys are metal, rough, trans, ior, density, emit, flux, media
    /// (0 absorb, 1 scatter, 2 emit, 3 sss) and g.
    pub fn palette_command(&mut self, line: &str, state: &egui_wgpu::RenderState) -> String {
        let words: Vec<&str> = line.split_whitespace().collect();
        let index = words.get(1).and_then(|w| w.parse::<u8>().ok()).filter(|&i| i > 0);
        let Some(index) = index else {
            return "palette <1-255> <r> <g> <b> | material <1-255> <kind> [key=value]...".to_owned();
        };
        self.history.end_gesture();
        let message = match words.as_slice() {
            ["palette", _, r, g, b] => match (r.parse(), g.parse(), b.parse()) {
                (Ok(r), Ok(g), Ok(b)) => {
                    self.set_colour(index, Color { r, g, b, a: 255 }, state);
                    format!("colour {index} = {r} {g} {b}")
                }
                _ => "palette <index> <r> <g> <b>, each 0-255".to_owned(),
            },
            ["material", _, kind, pairs @ ..] => {
                let Some(kind) = KINDS.iter().find(|(_, n)| n == kind).map(|(k, _)| *k) else {
                    return "kinds: diffuse, metal, glass, emit, cloud".to_owned();
                };
                let mut r = ResolvedMaterial { kind, ..Default::default() };
                start_kind(&mut r, kind);
                for pair in pairs {
                    let Some((k, v)) = pair.split_once('=') else { return format!("expected key=value, got '{pair}'") };
                    let Ok(v) = v.parse::<f32>() else { return format!("'{v}' is not a number") };
                    match k {
                        "metal" => r.metalness = v.clamp(0.0, 1.0),
                        "rough" => r.roughness = v.clamp(0.0, 1.0),
                        "trans" => r.transparency = v.clamp(0.0, 1.0),
                        "ior" => r.ior = v.clamp(1.0, 3.0),
                        "density" => r.density = v.max(0.0),
                        "emit" => r.emission = v.clamp(0.0, 1.0),
                        "flux" => r.flux = v.max(0.0),
                        "media" => {
                            r.media = MEDIA.get(v.round().clamp(0.0, 3.0) as usize).map_or(MediaKind::Absorb, |m| m.0)
                        }
                        "g" => r.phase = v.clamp(-0.99, 0.99),
                        _ => return format!("unknown key '{k}'"),
                    }
                }
                self.set_material(index, Some(r), state);
                format!("material {index}: {:?}", kind)
            }
            _ => "palette <index> <r> <g> <b> | material <index> <kind> [key=value]...".to_owned(),
        };
        // A command is a gesture of its own.
        self.history.end_gesture();
        message
    }
}
