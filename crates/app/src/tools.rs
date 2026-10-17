//! Mouse strokes: turning presses and drags in the viewport into brush edits.
//!
//! A stroke starts where a press hits a voxel and ends on release. Voxel
//! strokes write as they go, all merged into one undo step. Box and line
//! strokes are previewed: the preview is applied straight to the document,
//! outside the history, and swapped each time the cursor moves to a new cell;
//! on release it is replaced by one real, undoable command.

use crate::{App, edit::Ray};
use glam::{IVec3, Vec3};
use voxel_core::brush::{self, BrushMode, BrushShape};
use voxel_core::{Command, Hit};

/// A stroke in progress.
pub struct Stroke {
    instance: usize,
    model: usize,
    hit: Hit,
    /// The cell the dragged shapes currently end on.
    end: IVec3,
    /// What is applied to the document for display, outside the history.
    preview: Option<Command>,
}

impl App {
    /// Starts a stroke if the ray hits a voxel (or, attaching, the floor of
    /// the selected model; see `brush_pick`). Returns whether it did, so
    /// a press on empty space can orbit the camera instead.
    pub fn begin_stroke(&mut self, ray: Ray) -> bool {
        if self.picking {
            return self.pick_colour(ray);
        }
        let Some(pick) = self.brush_pick(ray) else {
            return false;
        };
        let Some(loaded) = &mut self.scene else { return false };
        self.selected = pick.instance;
        let model = loaded.doc.scene.instances[pick.instance].model_index;
        // One click does the whole job, and starts no stroke.
        match self.brush_shape {
            BrushShape::Shader => {
                let message = self.shader_brush();
                self.console_log.push(message);
                return true;
            }
            BrushShape::Pattern => {
                let volume = loaded.doc.scene.models[model].clone();
                let Some(writes) = self.pattern_stroke_writes(&volume, &pick.hit) else {
                    self.console_log.push("pattern brush: no pattern yet (make one from a selection)".to_owned());
                    return true;
                };
                let Some(loaded) = &mut self.scene else { return true };
                self.history.end_gesture();
                if self.history.set_voxels(&mut loaded.doc, model, writes) > 0 {
                    self.mesh_dirty = true;
                }
                self.history.end_gesture();
                return true;
            }
            _ => {}
        }
        let anchor = brush::anchor(self.brush_mode, &pick.hit);

        self.stroke = Some(Stroke {
            instance: pick.instance,
            model,
            hit: pick.hit,
            end: anchor,
            preview: None,
        });

        self.history.end_gesture();
        if self.brush_shape.is_dragged() {
            self.update_preview(anchor);
        } else {
            self.apply_at(pick.hit, anchor);
        }
        true
    }

    /// Follows the cursor while the button is held.
    pub fn update_stroke(&mut self, ray: Ray) {
        let Some(stroke) = &self.stroke else { return };
        match self.brush_shape {
            BrushShape::Box | BrushShape::Line | BrushShape::Square | BrushShape::Circle => {
                if let Some(end) = self.project_to_stroke_plane(ray)
                    && end != stroke.end
                {
                    self.update_preview(end);
                }
            }
            BrushShape::Voxel => {
                // Attaching follows the plane the stroke started on: picking
                // afresh would hit the voxel just added and stack toward the
                // camera. Erasing and painting follow the surface.
                if self.brush_mode == BrushMode::Attach {
                    if let Some(cell) = self.project_to_stroke_plane(ray)
                        && cell != stroke.end
                    {
                        let hit = stroke.hit;
                        self.apply_at(hit, cell);
                    }
                } else if let Some((instance, hit)) = self.pick_visible(ray)
                    && instance == stroke.instance && hit.voxel != stroke.end
                {
                    self.apply_at(hit, hit.voxel);
                }
            }
            // One click does the whole job.
            BrushShape::Face | BrushShape::Fill | BrushShape::Shader | BrushShape::Pattern => {}
        }
    }

    /// Commits the stroke on release.
    pub fn finish_stroke(&mut self) {
        let Some(stroke) = self.stroke.take() else { return };
        if let (Some(preview), Some(loaded)) = (stroke.preview, self.scene.as_mut()) {
            preview.revert(&mut loaded.doc);
            if let Command::SetVoxels { model, changes } = preview {
                let writes = changes.into_iter().map(|(p, _, next)| (p, next));
                let command = Command::set_voxels(&loaded.doc, model, writes);
                self.history.exec_alone(&mut loaded.doc, command);
            }
            self.mesh_dirty = true;
        }
        self.history.end_gesture();
    }

    /// Writes the stroke's shape at `cell` straight into the history; the
    /// open gesture merges successive calls into one undo step.
    fn apply_at(&mut self, hit: Hit, cell: IVec3) {
        let Some(stroke) = &mut self.stroke else { return };
        let Some(loaded) = &mut self.scene else { return };
        let model = stroke.model;
        let Some(volume) = loaded.doc.scene.models.get(model) else { return };

        // The voxel brush paints at `cell`, which may be off the clicked face
        // when dragging; the other shapes start from the hit itself.
        let axis = brush::face_axis(hit.face);
        let outward = if self.brush_mode == BrushMode::Attach { hit.face.offset()[axis] } else { 0 };
        let cells = match self.brush_shape {
            BrushShape::Voxel => brush::tip_cells(cell, axis, outward, &self.brush_options),
            BrushShape::Fill if self.brush_mode != BrushMode::Attach => brush::region(volume, hit.voxel, self.region),
            shape => brush::stroke_cells(volume, self.brush_mode, shape, &hit, cell, &self.brush_options),
        };
        let cells = brush::through_axes(&cells, volume.size(), self.axis_lock);
        let cells = brush::mirrored(&cells, volume.size(), self.mirror);
        let writes = if self.brush_shape == BrushShape::Face
            && self.brush_mode == BrushMode::Attach
            && self.brush_options.face_keep_colour
        {
            // Extruding in each voxel's own colour: the colour of the voxel
            // each new cell stands on.
            let o = hit.face.offset();
            let back = IVec3::new(o[0], o[1], o[2]);
            let coloured: Vec<(IVec3, u8)> = cells
                .iter()
                .map(|&p| {
                    let under = p - back;
                    let c = volume.get_i(under.x, under.y, under.z);
                    (p, if c == 0 { self.brush_color } else { c })
                })
                .collect();
            brush::pattern_writes(volume, self.brush_mode, &coloured)
        } else {
            brush::writes(volume, self.brush_mode, self.brush_color, &cells)
        };
        stroke.end = cell;
        if self.history.set_voxels(&mut loaded.doc, model, writes) > 0 {
            self.mesh_dirty = true;
        }
    }

    /// Replaces the preview with the shape ending at `end`.
    fn update_preview(&mut self, end: IVec3) {
        let Some(stroke) = &mut self.stroke else { return };
        let Some(loaded) = &mut self.scene else { return };
        if let Some(old) = stroke.preview.take() {
            old.revert(&mut loaded.doc);
        }
        let Some(volume) = loaded.doc.scene.models.get(stroke.model) else { return };
        let end = clamp_to(volume.size(), end);
        let cells = brush::stroke_cells(volume, self.brush_mode, self.brush_shape, &stroke.hit, end, &self.brush_options);
        let cells = brush::through_axes(&cells, volume.size(), self.axis_lock);
        let cells = brush::mirrored(&cells, volume.size(), self.mirror);
        let writes = brush::writes(volume, self.brush_mode, self.brush_color, &cells);
        let command = Command::set_voxels(&loaded.doc, stroke.model, writes);
        command.apply(&mut loaded.doc);
        stroke.preview = Some(command);
        stroke.end = end;
        self.mesh_dirty = true;
    }

    /// Where the ray crosses the plane through the stroke's anchor, parallel
    /// to the face it started on, as a cell in the model's own coordinates.
    fn project_to_stroke_plane(&self, ray: Ray) -> Option<IVec3> {
        let stroke = self.stroke.as_ref()?;
        let loaded = self.scene.as_ref()?;
        let instance = loaded.doc.scene.instances.get(stroke.instance)?;
        let volume = loaded.doc.scene.models.get(stroke.model)?;

        let origin = instance.transform.inverse_apply(ray.origin) + volume.pivot();
        let direction = instance.transform.inverse_rotate(ray.direction);
        let axis = brush::face_axis(stroke.hit.face);
        let anchor = brush::anchor(self.brush_mode, &stroke.hit);

        if direction[axis].abs() < 1e-6 {
            return None;
        }
        let plane = anchor[axis] as f32 + 0.5;
        let t = (plane - origin[axis]) / direction[axis];
        if t <= 0.0 {
            return None;
        }
        let mut cell = (origin + direction * t).floor().as_ivec3();
        cell[axis] = anchor[axis];
        Some(cell)
    }

    fn pick_visible(&self, ray: Ray) -> Option<(usize, Hit)> {
        let loaded = self.scene.as_ref()?;
        let visible = self.visibility();
        let pick = loaded
            .doc
            .scene
            .pick(ray.origin, ray.direction, crate::edit::PICK_RANGE, |i| visible.shows(i))?;
        Some((pick.instance, pick.hit))
    }

    /// The selected model's box and the grid on its floor, as line
    /// segments: where the attach brush lands when it misses every voxel,
    /// and the only way to see an empty model at all.
    pub fn model_guides(&self) -> Vec<crate::renderer::LineVertex> {
        const FRAME: [f32; 4] = [1.0, 1.0, 1.0, 0.35];
        const GRID: [f32; 4] = [1.0, 1.0, 1.0, 0.08];
        let Some(loaded) = &self.scene else { return Vec::new() };
        let scene = &loaded.doc.scene;
        let visible = self.visibility();
        let Some(instance) = scene.instances.get(self.selected).filter(|i| visible.shows(i)) else {
            return Vec::new();
        };
        let Some(volume) = scene.models.get(instance.model_index) else { return Vec::new() };
        let size = Vec3::from_array(volume.size().map(|s| s as f32));
        let world = |p: Vec3| instance.transform.apply(p - volume.pivot()).to_array();
        let mut out = Vec::new();
        let mut segment = |a: Vec3, b: Vec3, color: [f32; 4]| {
            out.push(crate::renderer::LineVertex { position: world(a), color });
            out.push(crate::renderer::LineVertex { position: world(b), color });
        };
        if self.view.grid {
            for x in 1..volume.size()[0] {
                segment(Vec3::new(x as f32, 0.0, 0.0), Vec3::new(x as f32, size.y, 0.0), GRID);
            }
            for y in 1..volume.size()[1] {
                segment(Vec3::new(0.0, y as f32, 0.0), Vec3::new(size.x, y as f32, 0.0), GRID);
            }
        }
        let corner = |i: usize| Vec3::new(
            if i & 1 == 0 { 0.0 } else { size.x },
            if i & 2 == 0 { 0.0 } else { size.y },
            if i & 4 == 0 { 0.0 } else { size.z },
        );
        if self.view.frame {
            for (a, b) in crate::CUBE_EDGES {
                segment(corner(a), corner(b), FRAME);
            }
        }
        out
    }

    /// The eyedropper: takes the clicked voxel's colour for the brush and
    /// goes back to the brush. A click on empty space leaves it waiting.
    fn pick_colour(&mut self, ray: Ray) -> bool {
        let Some((instance, hit)) = self.pick_visible(ray) else { return false };
        let Some(loaded) = &self.scene else { return false };
        let scene = &loaded.doc.scene;
        let Some(volume) = scene.instances.get(instance).and_then(|i| scene.models.get(i.model_index)) else {
            return false;
        };
        let colour = volume.get_i(hit.voxel.x, hit.voxel.y, hit.voxel.z);
        if colour == 0 {
            return false;
        }
        self.brush_color = colour;
        self.picking = false;
        self.console_log.push(format!("picked colour {colour}"));
        true
    }

    /// What the brush acts on under a ray: the nearest visible voxel, or,
    /// missing every voxel, the floor of the selected model's box when
    /// attaching -- so an empty model can be started with the mouse. Only
    /// that model's floor, so a click elsewhere still orbits; and only for
    /// shapes that place cells, not face and fill, which grow from an
    /// existing voxel.
    fn brush_pick(&self, ray: Ray) -> Option<voxel_core::ScenePick> {
        let loaded = self.scene.as_ref()?;
        let visible = self.visibility();
        let scene = &loaded.doc.scene;
        scene.pick(ray.origin, ray.direction, crate::edit::PICK_RANGE, |i| visible.shows(i)).or_else(|| {
            let places = matches!(self.brush_shape, BrushShape::Voxel | BrushShape::Box | BrushShape::Line | BrushShape::Square | BrushShape::Circle);
            let shown = scene.instances.get(self.selected).is_some_and(|i| visible.shows(i));
            if self.brush_mode == BrushMode::Attach && places && shown && !self.selecting && !self.moving && !self.picking {
                scene.pick_floor(self.selected, ray.origin, ray.direction)
            } else {
                None
            }
        })
    }

    /// The world-space corners of the cell the brush would act on under the
    /// cursor, for the hover outline.
    pub fn hover_cell(&self, ray: Ray) -> Option<[Vec3; 8]> {
        let loaded = self.scene.as_ref()?;
        let pick = self.brush_pick(ray)?;
        let (instance, hit) = (pick.instance, pick.hit);
        let instance = loaded.doc.scene.instances.get(instance)?;
        let volume = loaded.doc.scene.models.get(instance.model_index)?;
        let cell = if self.picking { hit.voxel } else { brush::anchor(self.brush_mode, &hit) }.as_vec3();
        let mut corners = [Vec3::ZERO; 8];
        for (i, c) in corners.iter_mut().enumerate() {
            let offset = Vec3::new((i & 1) as f32, ((i >> 1) & 1) as f32, ((i >> 2) & 1) as f32);
            *c = instance.transform.apply(cell + offset - volume.pivot());
        }
        Some(corners)
    }

    /// `brush [attach|erase|paint|select] [voxel|box|line|face|fill]`,
    /// `color <n>`.
    pub fn brush_command(&mut self, line: &str) -> String {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("color") => match words.next().and_then(|w| w.parse::<u8>().ok()) {
                Some(c) if c > 0 => {
                    self.brush_color = c;
                    format!("color {c}")
                }
                _ => "color <1-255>".to_owned(),
            },
            Some("brush") if line.split_whitespace().nth(1) == Some("mirror") => {
                match line.split_whitespace().nth(2) {
                    Some("off") => self.mirror = [false; 3],
                    Some(axes) if !axes.is_empty() && axes.chars().all(|c| "xyz".contains(c)) => {
                        self.mirror = ['x', 'y', 'z'].map(|a| axes.contains(a));
                    }
                    _ => return "brush mirror x|y|z|xy|xz|yz|xyz|off".to_owned(),
                }
                let on: String = ['x', 'y', 'z'].iter().zip(self.mirror).filter(|(_, m)| *m).map(|(a, _)| *a).collect();
                format!("mirror {}", if on.is_empty() { "off" } else { &on })
            }
            Some("brush") if matches!(line.split_whitespace().nth(1), Some("size" | "tip" | "centre" | "center" | "project" | "face")) => {
                self.brush_option_command(line)
            }
            Some("brush") if line.split_whitespace().nth(1) == Some("axis") => {
                match line.split_whitespace().nth(2) {
                    Some("off") => self.axis_lock = [false; 3],
                    Some(axes) if !axes.is_empty() && axes.chars().all(|c| "xyz".contains(c)) => {
                        self.axis_lock = ['x', 'y', 'z'].map(|a| axes.contains(a));
                    }
                    _ => return "brush axis x|y|z|xy|xz|yz|xyz|off".to_owned(),
                }
                let on: String = ['x', 'y', 'z'].iter().zip(self.axis_lock).filter(|(_, m)| *m).map(|(a, _)| *a).collect();
                format!("axis {}", if on.is_empty() { "off" } else { &on })
            }
            Some("brush") if line.split_whitespace().nth(1) == Some("region") => {
                let words: Vec<&str> = line.split_whitespace().skip(2).collect();
                let wide = words.contains(&"8");
                self.region = match words.first() {
                    Some(&"same") => brush::Region::SameColour,
                    Some(&"color" | &"colour") => brush::Region::Connected { by_colour: true, wide },
                    Some(&"geo") => brush::Region::Connected { by_colour: false, wide },
                    _ => return "brush region same | color [4|8] | geo [4|8]".to_owned(),
                };
                format!("region {}", region_name(self.region))
            }
            Some("brush") if line.split_whitespace().nth(1) == Some("pick") => {
                self.picking = true;
                "click a voxel to take its colour".to_owned()
            }
            _ => {
                for word in words {
                    if word == "select" {
                        self.selecting = true;
                        self.moving = false;
                    } else if word == "move" {
                        self.moving = true;
                        self.selecting = false;
                    } else if let Some(mode) = BrushMode::parse(word) {
                        self.brush_mode = mode;
                        self.selecting = false;
                        self.moving = false;
                    } else if let Some(shape) = BrushShape::parse(word) {
                        self.brush_shape = shape;
                    } else {
                        return format!(
                            "unknown brush '{word}': attach, erase, paint, select, move, pick; \
                             voxel, box, line, face, fill, shader, pattern; mirror <axes>; axis <axes>; region …"
                        );
                    }
                }
                let mode = if self.moving {
                    "move"
                } else if self.selecting {
                    "select"
                } else {
                    self.brush_mode.name()
                };
                format!("brush {mode} {}", self.brush_shape.name())
            }
        }
    }
}

impl App {
    /// `brush size <1-64>`, `brush tip cube|sphere|2d|3d`, `brush centre
    /// fill|ring|even|odd`, `brush project on|off`, `brush face
    /// color|geo|4|8|palette|voxel`.
    fn brush_option_command(&mut self, line: &str) -> String {
        let words: Vec<&str> = line.split_whitespace().skip(1).collect();
        let o = &mut self.brush_options;
        let ok = match words.as_slice() {
            ["size", n] => match n.parse::<u32>() {
                Ok(n) if (1..=64).contains(&n) => {
                    o.size = n;
                    true
                }
                _ => false,
            },
            ["tip", w] => match *w {
                "cube" => { o.sphere = false; true }
                "sphere" => { o.sphere = true; true }
                "2d" => { o.flat = true; true }
                "3d" => { o.flat = false; true }
                _ => false,
            },
            ["centre" | "center", w] => match *w {
                "fill" => { o.fill_centre = true; true }
                "ring" => { o.fill_centre = false; true }
                "even" => { o.even = true; true }
                "odd" => { o.even = false; true }
                _ => false,
            },
            ["project", w] => match *w {
                "on" => { o.project_line = true; true }
                "off" => { o.project_line = false; true }
                _ => false,
            },
            ["face", w] => match *w {
                "color" | "colour" => { o.face_by_colour = true; true }
                "geo" => { o.face_by_colour = false; true }
                "4" => { o.face_wide = false; true }
                "8" => { o.face_wide = true; true }
                "palette" => { o.face_keep_colour = false; true }
                "voxel" => { o.face_keep_colour = true; true }
                _ => false,
            },
            _ => false,
        };
        if !ok {
            return "brush size <1-64> | tip cube|sphere|2d|3d | centre fill|ring|even|odd | project on|off | face color|geo|4|8|palette|voxel".to_owned();
        }
        let o = self.brush_options;
        format!(
            "brush size {}, {} {}, {} {}, project {}, face {} {} {}",
            o.size,
            if o.sphere { "sphere" } else { "cube" },
            if o.flat { "2d" } else { "3d" },
            if o.fill_centre { "fill" } else { "ring" },
            if o.even { "even" } else { "odd" },
            if o.project_line { "on" } else { "off" },
            if o.face_by_colour { "color" } else { "geo" },
            if o.face_wide { 8 } else { 4 },
            if o.face_keep_colour { "voxel" } else { "palette" },
        )
    }

    /// The options of the chosen shape, under the shape buttons.
    pub fn brush_options_ui(&mut self, ui: &mut eframe::egui::Ui) {
        use eframe::egui;
        let o = &mut self.brush_options;
        match self.brush_shape {
            BrushShape::Voxel => {
                ui.horizontal_wrapped(|ui| {
                    ui.add(egui::Slider::new(&mut o.size, 1..=64).text("size")).on_hover_text("- and + change it");
                    if ui.selectable_label(!o.sphere, "Cube").clicked() {
                        o.sphere = false;
                    }
                    if ui.selectable_label(o.sphere, "Sphere").clicked() {
                        o.sphere = true;
                    }
                    if ui.selectable_label(o.flat, "2D").on_hover_text("Flat on the face").clicked() {
                        o.flat = true;
                    }
                    if ui.selectable_label(!o.flat, "3D").clicked() {
                        o.flat = false;
                    }
                });
            }
            BrushShape::Line => {
                ui.checkbox(&mut o.project_line, "Project line on the surface");
            }
            BrushShape::Square | BrushShape::Circle => {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut o.fill_centre, "Fill centre");
                    ui.checkbox(&mut o.even, "Even").on_hover_text("An even size, centred on a corner");
                });
            }
            BrushShape::Face => {
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(o.face_by_colour, "Colour").on_hover_text("Connected voxels of the clicked colour").clicked() {
                        o.face_by_colour = true;
                    }
                    if ui.selectable_label(!o.face_by_colour, "Geo").on_hover_text("Connected voxels of any colour").clicked() {
                        o.face_by_colour = false;
                    }
                    if ui.selectable_label(!o.face_wide, "4").clicked() {
                        o.face_wide = false;
                    }
                    if ui.selectable_label(o.face_wide, "8").on_hover_text("Through corners too").clicked() {
                        o.face_wide = true;
                    }
                    ui.checkbox(&mut o.face_keep_colour, "Voxel colour")
                        .on_hover_text("Extrude each voxel in its own colour, not the brush's");
                });
            }
            _ => {}
        }
    }

    /// - and + resize the voxel brush, as in the reference editor.
    pub fn brush_input(&mut self, ctx: &eframe::egui::Context) {
        use eframe::egui::Key;
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let (down, up) = ctx.input(|i| (i.key_pressed(Key::Minus), i.key_pressed(Key::Equals) || i.key_pressed(Key::Plus)));
        let size = &mut self.brush_options.size;
        if down {
            *size = size.saturating_sub(1).max(1);
        }
        if up {
            *size = (*size + 1).min(64);
        }
    }
}

/// How the region rule reads in messages and the panel.
pub fn region_name(region: brush::Region) -> String {
    match region {
        brush::Region::SameColour => "same colour".to_owned(),
        brush::Region::Connected { by_colour, wide } => format!(
            "connected by {}, {} neighbours",
            if by_colour { "colour" } else { "geometry" },
            if wide { 8 } else { 4 }
        ),
    }
}

fn clamp_to(size: [u32; 3], p: IVec3) -> IVec3 {
    let max = IVec3::new(size[0] as i32, size[1] as i32, size[2] as i32) - IVec3::ONE;
    p.clamp(IVec3::ZERO, max.max(IVec3::ZERO))
}
