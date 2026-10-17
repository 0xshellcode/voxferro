//! The window's layout and look, after the reference editor's:
//!
//! - along the top, Model and Render on the left, the project's name in the
//!   middle, undo, redo and the file actions on the right;
//! - on the left, the Palette column (eight colours wide, the colour editor
//!   folded under it) and the Brush column (modes, attach/erase/paint, the
//!   tools, mirror and axis, and the display switches at its foot) -- or, in
//!   Render mode, the render settings;
//! - over the viewport, a bar with the model's size, crop and fit;
//! - under it, the console bar: the console, the camera modes, a snapshot,
//!   and a drawer with the log and the camera views;
//! - on the right, the Edit panel and the Project panel (`model_edit`,
//!   `project`).
//!
//! The look: the reference editor's greys, flat buttons that fill their column, panel
//! titles centred, sections in small capitals.

use crate::App;
use eframe::{egui, egui_wgpu};

/// the reference editor's palette of greys, and its blue for what is on.
pub const PANEL: egui::Color32 = egui::Color32::from_rgb(43, 43, 45);
const FIELD: egui::Color32 = egui::Color32::from_rgb(30, 30, 31);
const BUTTON: egui::Color32 = egui::Color32::from_rgb(78, 78, 82);
const BUTTON_HOVER: egui::Color32 = egui::Color32::from_rgb(96, 96, 101);
const BUTTON_ACTIVE: egui::Color32 = egui::Color32::from_rgb(112, 112, 118);
pub const ACCENT: egui::Color32 = egui::Color32::from_rgb(110, 170, 240);
const TEXT: egui::Color32 = egui::Color32::from_rgb(214, 214, 218);
const TEXT_DIM: egui::Color32 = egui::Color32::from_rgb(150, 150, 156);

/// Icons: Phosphor's regular set, mixed into ordinary text.
pub use egui_phosphor::regular as icon;

/// The theme, set once at start.
pub fn apply_theme(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    ctx.set_fonts(fonts);
    let mut style = (*ctx.global_style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = FIELD;
    v.faint_bg_color = egui::Color32::from_rgb(50, 50, 53);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = egui::Color32::from_rgb(58, 96, 140);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    let round = egui::CornerRadius::same(3);
    for (w, fill) in [
        (&mut v.widgets.inactive, BUTTON),
        (&mut v.widgets.hovered, BUTTON_HOVER),
        (&mut v.widgets.active, BUTTON_ACTIVE),
        (&mut v.widgets.open, BUTTON_HOVER),
    ] {
        w.weak_bg_fill = fill;
        w.bg_fill = fill;
        w.corner_radius = round;
        w.bg_stroke = egui::Stroke::NONE;
        w.fg_stroke = egui::Stroke::new(1.0, TEXT);
    }
    v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(58, 58, 61));
    v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_DIM);
    v.widgets.noninteractive.corner_radius = round;
    let sp = &mut style.spacing;
    sp.item_spacing = egui::vec2(3.0, 3.0);
    sp.button_padding = egui::vec2(6.0, 3.0);
    sp.interact_size.y = 22.0;
    use egui::{FontFamily::Proportional, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Heading, FontId::new(15.0, Proportional)),
        (TextStyle::Body, FontId::new(12.5, Proportional)),
        (TextStyle::Button, FontId::new(12.5, Proportional)),
        (TextStyle::Small, FontId::new(10.5, Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, egui::FontFamily::Monospace)),
    ]
    .into();
    ctx.set_global_style(style);
}

/// A panel's title, centred and dim, as the reference editor's.
pub fn title(ui: &mut egui::Ui, text: &str) {
    ui.vertical_centered(|ui| ui.label(egui::RichText::new(text).size(13.5).color(TEXT_DIM)));
    ui.add_space(2.0);
}

/// A small-capitals section label.
pub fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).size(10.5).color(TEXT_DIM));
}

/// A button that is on or off, drawn lit when on.
pub fn toggle(ui: &mut egui::Ui, on: bool, text: &str, width: f32) -> egui::Response {
    let fill = if on { egui::Color32::from_rgb(58, 96, 140) } else { BUTTON };
    let narrow = width < 52.0;
    let text = egui::RichText::new(text).size(label_size(text, narrow)).color(if on { egui::Color32::WHITE } else { TEXT });
    // Narrow buttons give their padding to the label.
    ui.scope(|ui| {
        if narrow {
            ui.spacing_mut().button_padding.x = 1.0;
        }
        ui.add_sized([width, 22.0], egui::Button::new(text).fill(fill).truncate())
    })
    .inner
}

/// A label's size: an icon (a single glyph from the icon font) larger.
fn label_size(text: &str, narrow: bool) -> f32 {
    let mut chars = text.chars();
    let glyph = matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_ascii());
    if glyph {
        16.0
    } else if narrow {
        11.0
    } else {
        12.5
    }
}

/// An icon button with its name as the hover text.
pub fn icon_button(ui: &mut egui::Ui, glyph: &str, hint: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(egui::RichText::new(glyph).size(17.0)).min_size(egui::vec2(28.0, 24.0)))
        .on_hover_text(hint)
        .on_disabled_hover_text(hint)
}

/// A row of `n` equal buttons filling the width; returns which was clicked.
pub fn button_row(ui: &mut egui::Ui, labels: &[(&str, &str)]) -> Option<usize> {
    let n = labels.len().max(1) as f32;
    let width = (ui.available_width() - ui.spacing().item_spacing.x * (n - 1.0)) / n;
    let mut clicked = None;
    ui.horizontal(|ui| {
        for (i, (label, hint)) in labels.iter().enumerate() {
            let narrow = width < 52.0;
            let text = egui::RichText::new(*label).size(label_size(label, narrow));
            let mut b = ui
                .scope(|ui| {
                    if narrow {
                        ui.spacing_mut().button_padding.x = 1.0;
                    }
                    ui.add_sized([width, 22.0], egui::Button::new(text).truncate())
                })
                .inner;
            if !hint.is_empty() {
                b = b.on_hover_text(*hint);
            }
            if b.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// Lit buttons in a row filling the width; returns which was clicked.
pub fn toggle_row(ui: &mut egui::Ui, items: &[(bool, &str, &str)]) -> Option<usize> {
    let n = items.len().max(1) as f32;
    let width = (ui.available_width() - ui.spacing().item_spacing.x * (n - 1.0)) / n;
    let mut clicked = None;
    ui.horizontal(|ui| {
        for (i, (on, label, hint)) in items.iter().enumerate() {
            let mut b = toggle(ui, *on, label, width);
            if !hint.is_empty() {
                b = b.on_hover_text(*hint);
            }
            if b.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

impl App {
    /// Every panel, in the reference editor's places.
    pub fn layout(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        egui::Panel::top("top bar").show(ui, |ui| self.top_bar(ui, state));
        egui::Panel::bottom("console bar").show(ui, |ui| self.console_bar(ui, state));
        if self.rendering {
            egui::Panel::left("render column").exact_size(290.0).show(ui, |ui| {
                title(ui, "Render");
                egui::ScrollArea::vertical().id_salt("render scroll").show(ui, |ui| self.render_column(ui, state));
            });
        } else {
            egui::Panel::left("palette column").exact_size(178.0).show(ui, |ui| self.palette_column(ui, state));
            egui::Panel::left("brush column").exact_size(150.0).show(ui, |ui| self.brush_column(ui));
        }
        egui::Panel::right("project").exact_size(200.0).show(ui, |ui| self.project_panel(ui, state));
        if !self.rendering {
            egui::Panel::right("edit").exact_size(168.0).show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("edit scroll").show(ui, |ui| self.edit_panel(ui));
            });
        }
        // The viewport runs to the panels' edges, as the reference editor's.
        let frame = egui::Frame::NONE.fill(egui::Color32::from_rgb(38, 38, 40));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            if !self.rendering {
                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| self.viewport_bar(ui));
            }
            self.viewport(ui, state);
        });
        self.settings_window(ui.ctx(), state);
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        let ctx = ui.ctx().clone();
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            for (rendering, label) in [(false, "Model"), (true, "Render")] {
                let on = self.rendering == rendering;
                let text = egui::RichText::new(label).size(17.0).color(if on { ACCENT } else { TEXT_DIM });
                if ui.add(egui::Label::new(text).sense(egui::Sense::click())).clicked() && !on {
                    self.rendering = rendering;
                    self.path_tracer.reset();
                }
                ui.add_space(8.0);
            }
            // The file actions on the right, as the reference editor's top bar.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_doc = self.scene.is_some();
                let mut line = None;
                if icon_button(ui, icon::GEAR, "Settings, agents (MCP) and help", true).clicked() {
                    self.settings_open = !self.settings_open;
                }
                ui.separator();
                for (glyph, command, hint, enabled) in [
                    (icon::TRASH, "project delete", "Move the file to the trash", has_doc),
                    (icon::COPY, "project dup", "Duplicate: save a copy as name-1.vox and go on there", has_doc),
                    (icon::FILE_PLUS, "new", "New model (Cmd+N)", true),
                    (icon::FILE_ARROW_DOWN, "import", "Import the models of a .vox, XRAW, QB, SLAB6 or OBJ file", has_doc),
                    (icon::DOWNLOAD_SIMPLE, "save as", "Save as (Cmd+Shift+S)", has_doc),
                    (icon::FLOPPY_DISK, "save", "Save (Cmd+S)", has_doc),
                    (icon::FOLDER_OPEN, "open", "Open (Cmd+O)", true),
                ] {
                    if icon_button(ui, glyph, hint, enabled).clicked() {
                        line = Some(command);
                    }
                }
                ui.separator();
                if icon_button(ui, icon::ARROW_CLOCKWISE, "Redo (Cmd+Shift+Z)", self.history.can_redo()).clicked() {
                    self.redo(state);
                }
                if icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, "Undo (Cmd+Z)", self.history.can_undo()).clicked() {
                    self.undo(state);
                }
                // The project's name in what is left, centred.
                let name = self.scene.as_ref().map(|s| s.name.clone()).unwrap_or_default();
                let dirty = if self.is_dirty() { " ●" } else { "" };
                ui.with_layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight), |ui| {
                    let summary = self.scene.as_ref().map(|s| {
                        format!(
                            "{} placed models, {} voxels, {} triangles",
                            s.doc.scene.instances.len(),
                            s.doc.scene.total_voxels(),
                            self.scene_renderer.triangle_count()
                        )
                    });
                    let label = ui.label(egui::RichText::new(format!("{}{dirty}", name.trim_end_matches(".vox"))).monospace().size(14.0).color(TEXT_DIM));
                    if let Some(summary) = summary {
                        label.on_hover_text(summary);
                    }
                });
                match line {
                    Some("save") => {
                        if let Some(m) = self.save() {
                            self.console_log.push(m);
                        }
                    }
                    Some("save as") => {
                        if let Some(m) = self.save_as() {
                            self.console_log.push(m);
                        }
                    }
                    Some("open") => self.request(crate::files::Discarding::Open(None), &ctx, state),
                    Some("new") => self.request(crate::files::Discarding::New(crate::files::NEW_MODEL_SIZE), &ctx, state),
                    Some(other) => self.run_command(other, &ctx, state),
                    None => {}
                }
            });
        });
        ui.add_space(2.0);
    }

    fn palette_column(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        title(ui, "Palette");
        // the reference editor's four default palettes, when an install has them.
        let mut line = None;
        let defaults: Vec<u8> = (0..4u8).filter(|n| crate::palette_files::reference_editor_palette(*n).is_some()).collect();
        if !defaults.is_empty() {
            let labels: Vec<(String, String)> = defaults.iter().map(|n| (n.to_string(), format!("the reference editor's default palette {n}"))).collect();
            let refs: Vec<(&str, &str)> = labels.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
            if let Some(i) = button_row(ui, &refs) {
                line = Some(format!("palette default {}", defaults[i]));
            }
        }
        self.palette_grid(ui);
        if let Some(i) = button_row(
            ui,
            &[(icon::FOLDER_OPEN, "Open a palette image (PNG)"), (icon::FLOPPY_DISK, "Save the palette as a 256 × 1 PNG"), (icon::FILE, "New: the format's default palette")],
        ) {
            match i {
                0 => {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Palette", &["png"]).pick_file() {
                        line = Some(format!("palette open {}", path.display()));
                    }
                }
                1 => {
                    if let Some(path) = rfd::FileDialog::new().add_filter("Palette", &["png"]).set_file_name("palette.png").save_file() {
                        line = Some(format!("palette save {}", path.display()));
                    }
                }
                _ => line = Some("palette reset".to_owned()),
            }
        }
        if let Some(line) = line {
            let message = self.palette_file_command(&line, state).unwrap_or_default();
            self.console_log.push(message);
        }
        egui::CollapsingHeader::new(egui::RichText::new("COLOR").size(10.5)).id_salt("color editor").show(ui, |ui| {
            self.palette_editor_ui(ui, state);
        });
    }

    fn brush_column(&mut self, ui: &mut egui::Ui) {
        use voxel_core::{BrushMode, BrushShape};
        title(ui, "Brush");
        // The display switches at the column's foot, as the reference editor's.
        egui::Panel::bottom("display switches").show(ui, |ui| {
            ui.add_space(3.0);
            let v = self.view;
            let mut word = None;
            for pair in [[("bg", "BO", v.background, "Background objects (Cmd+B)"), ("edges", "Edge", v.edges, "Voxel edges (Cmd+E)")],
                [("wire", "WR", v.wire, "Wireframe (Cmd+M)"), ("grid", "Grid", v.grid, "Floor grid (Cmd+G)")],
                [("shadow", "SW", v.shadow, "Shadows"), ("frame", "Frame", v.frame, "The model's box (Cmd+F)")],
                [("cube", "Cube", self.view_cube_shown, "The view cube"), ("ruler", "Ruler", self.ruler_shown, "Camera ruler: drag on it to turn in 5° steps")]]
            {
                let items: Vec<(bool, &str, &str)> = pair.iter().map(|(_, l, on, h)| (*on, *l, *h)).collect();
                if let Some(i) = toggle_row(ui, &items) {
                    word = Some(pair[i].0);
                }
            }
            if let Some(w) = word {
                let message = self.view_command(&format!("view {w}"));
                self.console_log.push(message);
            }
        });
        egui::ScrollArea::vertical().id_salt("brush scroll").show(ui, |ui| {
            // Shapes, three a row.
            let shapes: [(BrushShape, &str, &str); 9] = [
                (BrushShape::Voxel, icon::CUBE, "Voxel: one voxel, or the tip's size"),
                (BrushShape::Box, icon::BOUNDING_BOX, "Box: drag a rectangle on the face"),
                (BrushShape::Line, icon::LINE_SEGMENT, "Line"),
                (BrushShape::Square, icon::SQUARE, "Square, from its centre"),
                (BrushShape::Circle, icon::CIRCLE, "Circle, from its centre"),
                (BrushShape::Face, icon::ARROW_LINE_UP, "Face: the flat region, extruded"),
                (BrushShape::Fill, icon::PAINT_BUCKET, "Fill: a region"),
                (BrushShape::Shader, icon::CODE, "Shader: the chosen voxel script"),
                (BrushShape::Pattern, icon::STAR, "Pattern: stamp the pattern"),
            ];
            for chunk in shapes.chunks(3) {
                let items: Vec<(bool, &str, &str)> = chunk.iter().map(|(s, g, h)| (self.brush_shape == *s, *g, *h)).collect();
                if let Some(i) = toggle_row(ui, &items) {
                    self.brush_shape = chunk[i].0;
                }
            }
            ui.add_space(4.0);
            // Attach, Erase, Paint: the reference editor's big stacked buttons.
            for (mode, name) in BrushMode::ALL {
                let on = !self.selecting && !self.moving && self.brush_mode == mode;
                if toggle(ui, on, &crate::capitalised(name), ui.available_width()).clicked() {
                    self.brush_mode = mode;
                    self.selecting = false;
                    self.moving = false;
                }
            }
            ui.add_space(4.0);
            let tools = [
                (self.moving, icon::ARROWS_OUT_CARDINAL, "Move: drag a model or group. Shift: up and down. Cmd: turn."),
                (self.selecting, icon::SELECTION, "Select: Shift adds, Option subtracts; the shape decides how"),
                (self.picking, icon::EYEDROPPER, "Pick the colour of the next voxel clicked (or Alt+click)"),
            ];
            if let Some(i) = toggle_row(ui, &tools) {
                match i {
                    0 => {
                        self.moving = !self.moving;
                        self.selecting = false;
                    }
                    1 => {
                        self.selecting = !self.selecting;
                        self.moving = false;
                    }
                    _ => self.picking = !self.picking,
                }
            }
            ui.add_space(4.0);
            caption(ui, &format!("{} MIRROR", icon::FLIP_HORIZONTAL));
            let m = self.mirror;
            if let Some(i) = toggle_row(ui, &[(m[0], "X", "Repeat strokes reflected"), (m[1], "Y", ""), (m[2], "Z", "")]) {
                self.mirror[i] = !self.mirror[i];
            }
            caption(ui, &format!("{} AXIS", icon::ARROWS_LEFT_RIGHT));
            let a = self.axis_lock;
            if let Some(i) = toggle_row(ui, &[(a[0], "X", "Strokes run through the whole model"), (a[1], "Y", ""), (a[2], "Z", "")]) {
                self.axis_lock[i] = !self.axis_lock[i];
            }
            ui.add_space(4.0);
            self.brush_options_ui(ui);
            self.brush_extra_ui(ui);
        });
    }

    /// Over the viewport: the model's size, crop and fit, as the reference editor's.
    fn viewport_bar(&mut self, ui: &mut egui::Ui) {
        let current = self.selected_model().and_then(|m| Some((m, self.scene.as_ref()?.doc.scene.models.get(m)?.size())));
        if current.is_some() && current != self.resize_for {
            self.resize_for = current;
            if let Some((_, size)) = current {
                self.resize_to = size;
            }
        }
        let mut line = None;
        ui.horizontal(|ui| {
            if icon_button(ui, icon::CROP, "Crop: fit the box to the voxels", true).clicked() {
                line = Some("crop".to_owned());
            }
            for v in &mut self.resize_to {
                ui.add(egui::DragValue::new(v).range(1..=256).speed(0.2));
            }
            if icon_button(ui, icon::CHECK, "Resize the model's box to these sizes", true).clicked() {
                let [x, y, z] = self.resize_to;
                line = Some(format!("resize {x} {y} {z}"));
            }
        });
        if let Some(line) = line {
            let ctx = ui.ctx().clone();
            let message = self.select_command(&line, &ctx);
            self.console_log.push(message);
        }
    }

    /// Under the viewport: the console, the camera modes and a snapshot,
    /// with a drawer for the log and the camera's views and slots.
    fn console_bar(&mut self, ui: &mut egui::Ui, state: &egui_wgpu::RenderState) {
        use crate::camera::Projection;
        if self.drawer_open {
            ui.add_space(4.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width() * 0.55);
                    caption(ui, "LOG");
                    egui::ScrollArea::vertical().max_height(110.0).id_salt("console log").stick_to_bottom(true).show(ui, |ui| {
                        for entry in self.console_log.iter().rev().take(60).collect::<Vec<_>>().into_iter().rev() {
                            ui.label(egui::RichText::new(entry).monospace().size(11.0));
                        }
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    caption(ui, "CAMERA");
                    self.camera_ui(ui);
                });
            });
            ui.separator();
        }
        ui.horizontal(|ui| {
            let caret = if self.drawer_open { icon::CARET_DOWN } else { icon::CARET_UP };
            if icon_button(ui, caret, "The log and the camera views", true).clicked() {
                self.drawer_open = !self.drawer_open;
            }
            if icon_button(ui, icon::CAMERA, "Snapshot of the viewport (snap)", true).clicked() {
                let message = self.snap(None, state);
                self.console_log.push(message);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut line = None;
                if icon_button(ui, icon::CORNERS_OUT, "Frame: the selection or the model (F; Shift+F all)", true).clicked() {
                    line = Some("camera frame");
                }
                for (p, label, command) in [
                    (Projection::Isometric, "Iso", "camera iso"),
                    (Projection::Orthographic, "Orth", "camera ortho"),
                    (Projection::Free, "Free", "camera free"),
                    (Projection::Perspective, "Pers", "camera persp"),
                ] {
                    let on = self.camera.projection == p;
                    let text = egui::RichText::new(label).size(14.0).color(if on { ACCENT } else { TEXT_DIM });
                    if ui.add(egui::Label::new(text).sense(egui::Sense::click())).clicked() {
                        line = Some(command);
                    }
                }
                if let Some(line) = line {
                    let message = self.camera_command(line);
                    self.console_log.push(message);
                }
                // The console fills what is left; its last answer shows as
                // the hint.
                let hint = self.console_log.last().cloned().unwrap_or_else(|| "console".to_owned());
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.console_input)
                        .hint_text(egui::RichText::new(hint).color(TEXT_DIM))
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width()),
                );
                if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let line = std::mem::take(&mut self.console_input);
                    let ctx = ui.ctx().clone();
                    self.run_command(&line, &ctx, state);
                    response.request_focus();
                }
            });
        });
    }

    /// The gear's window: the graphics backend, agents over MCP, and help.
    fn settings_window(&mut self, ctx: &egui::Context, _state: &egui_wgpu::RenderState) {
        let mut open = self.settings_open;
        egui::Window::new("Settings").open(&mut open).resizable(false).default_width(320.0).show(ctx, |ui| {
            ui.label(format!("Graphics: {}", self.backend));
            ui.separator();
            self.mcp_panel(ui);
            ui.separator();
            caption(ui, "MOUSE AND KEYS");
            for line in [
                "Drag on the model to edit, elsewhere (or right-drag) to orbit.",
                "Shift+drag or middle-drag pans; scroll zooms.",
                "Option+Shift+click selects a group (again: one level in).",
                "the reference editor's keys: V F B L C P shapes, T R G modes, M N select, 1 2 3 mirror, Cmd+1 2 3 axis, U full, I fill, 5 frame, Alt+click eyedropper; Alt+1-7 views, O ortho, - + brush size.",
                "Cmd+E/F/G/B/M display switches; Cmd+Z undo.",
                "Free camera: W A S D Q E fly.",
            ] {
                ui.label(egui::RichText::new(line).size(11.5));
            }
        });
        self.settings_open = open;
    }
}
