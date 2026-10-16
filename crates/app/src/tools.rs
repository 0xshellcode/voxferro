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
        let Some(pick) = self.brush_pick(ray) else {
            return false;
        };
        let Some(loaded) = &mut self.scene else { return false };
        self.selected = pick.instance;
        let model = loaded.doc.scene.instances[pick.instance].model_index;
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
            BrushShape::Box | BrushShape::Line => {
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
            BrushShape::Face | BrushShape::Fill => {}
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
        let cells = match self.brush_shape {
            BrushShape::Voxel => vec![cell],
            shape => brush::stroke_cells(volume, self.brush_mode, shape, &hit, cell),
        };
        let writes = brush::writes(volume, self.brush_mode, self.brush_color, &cells);
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
        let cells = brush::stroke_cells(volume, self.brush_mode, self.brush_shape, &stroke.hit, end);
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
            .pick(ray.origin, ray.direction, 10_000.0, |i| visible.shows(i))?;
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
        for x in 1..volume.size()[0] {
            segment(Vec3::new(x as f32, 0.0, 0.0), Vec3::new(x as f32, size.y, 0.0), GRID);
        }
        for y in 1..volume.size()[1] {
            segment(Vec3::new(0.0, y as f32, 0.0), Vec3::new(size.x, y as f32, 0.0), GRID);
        }
        let corner = |i: usize| Vec3::new(
            if i & 1 == 0 { 0.0 } else { size.x },
            if i & 2 == 0 { 0.0 } else { size.y },
            if i & 4 == 0 { 0.0 } else { size.z },
        );
        for (a, b) in crate::CUBE_EDGES {
            segment(corner(a), corner(b), FRAME);
        }
        out
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
        scene.pick(ray.origin, ray.direction, 10_000.0, |i| visible.shows(i)).or_else(|| {
            let places = matches!(self.brush_shape, BrushShape::Voxel | BrushShape::Box | BrushShape::Line);
            let shown = scene.instances.get(self.selected).is_some_and(|i| visible.shows(i));
            if self.brush_mode == BrushMode::Attach && places && shown && !self.selecting && !self.moving {
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
        let cell = brush::anchor(self.brush_mode, &hit).as_vec3();
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
                        return format!("unknown brush '{word}': attach, erase, paint, select, move; voxel, box, line, face, fill");
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

fn clamp_to(size: [u32; 3], p: IVec3) -> IVec3 {
    let max = IVec3::new(size[0] as i32, size[1] as i32, size[2] as i32) - IVec3::ONE;
    p.clamp(IVec3::ZERO, max.max(IVec3::ZERO))
}
