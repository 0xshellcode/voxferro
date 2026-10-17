//! The view cube, in the viewport's corner: a cube turned with the camera,
//! its faces named; a click on one goes to that view. the reference editor shows one
//! too (`lower.ui`'s "Show View Cube"); `view cube on|off`.
//!
//! Beside it, the camera ruler (`lower.ui`'s "Show Camera Ruler : Drag on
//! Ruler"; `view ruler on|off`). As the reference editor a recent version draws it
//! (`UI_3DView::GenRuler`, `MouseDrag`): not a graduated scale but two
//! strips on the outer edges of the cube's corner, one reading the camera's
//! turn and one its elevation, in whole degrees. A drag on a strip turns
//! the camera on that axis, the corner where they meet both; the angle
//! moves 180° for half the viewport's height dragged and lands on the
//! scene's 5° snap (`scene : (snap = '5')` in `editor.ui`), starting from
//! the current angle snapped. The turn reads 0 from the front.

use crate::App;
use crate::camera::View;
use eframe::egui;
use glam::Vec3;

/// The faces: outward normal, label, the view that looks at that face.
const FACES: [(Vec3, &str, View); 6] = [
    (Vec3::NEG_Y, "Front", View::Front),
    (Vec3::Y, "Back", View::Back),
    (Vec3::NEG_X, "Left", View::Left),
    (Vec3::X, "Right", View::Right),
    (Vec3::Z, "Top", View::Top),
    (Vec3::NEG_Z, "Bottom", View::Bottom),
];

/// The cube's side, its margin from the viewport's corner, and the
/// ruler's strips' width.
const CUBE: f32 = 84.0;
const MARGIN: f32 = 12.0;
const STRIP: f32 = 24.0;
/// Degrees the ruler's angles land on.
const SNAP: f32 = 5.0;

/// Where the cube sits in a viewport: in the corner, or inside the ruler.
pub fn cube_rect(viewport: egui::Rect, ruler: bool) -> egui::Rect {
    let shift = if ruler { STRIP + 4.0 } else { 0.0 };
    egui::Rect::from_min_size(viewport.right_bottom() - egui::vec2(MARGIN + CUBE + shift, MARGIN + CUBE + shift), egui::vec2(CUBE, CUBE))
}

/// The ruler's parts: the turn's strip along the bottom, the elevation's
/// along the right, and the corner where they meet.
pub fn ruler_rects(viewport: egui::Rect) -> [egui::Rect; 3] {
    let max = viewport.right_bottom() - egui::vec2(MARGIN, MARGIN);
    let min = cube_rect(viewport, true).min;
    [
        egui::Rect::from_min_max(egui::pos2(min.x, max.y - STRIP), egui::pos2(max.x - STRIP, max.y)),
        egui::Rect::from_min_max(egui::pos2(max.x - STRIP, min.y), egui::pos2(max.x, max.y - STRIP)),
        egui::Rect::from_min_max(max - egui::vec2(STRIP, STRIP), max),
    ]
}

/// The area the corner's widgets take, which the viewport leaves to them.
pub fn corner_rect(viewport: egui::Rect, cube: bool, ruler: bool) -> Option<egui::Rect> {
    if ruler {
        Some(cube_rect(viewport, true).union(ruler_rects(viewport)[2]))
    } else {
        cube.then(|| cube_rect(viewport, false))
    }
}

/// The camera's turn as the ruler reads it: degrees from the front view,
/// -179 to 180.
pub fn ruler_yaw(yaw: f32) -> f32 {
    let d = yaw.to_degrees() + 90.0;
    180.0 - (180.0 - d).rem_euclid(360.0)
}

/// The angle on the snap grid.
fn snapped(degrees: f32) -> f32 {
    (degrees / SNAP).round() * SNAP
}

/// A drag on the ruler: which angles it turns, and where each would be
/// without the snap.
#[derive(Debug, Clone, Copy)]
pub struct RulerDrag {
    yaw: Option<f32>,
    pitch: Option<f32>,
}

impl RulerDrag {
    /// Starts at the current angles, snapped.
    pub fn start(yaw: bool, pitch: bool, camera: &crate::camera::OrbitCamera) -> Self {
        Self {
            yaw: yaw.then(|| snapped(ruler_yaw(camera.yaw))),
            pitch: pitch.then(|| snapped(camera.pitch.to_degrees())),
        }
    }

    /// Moves by a drag of `delta` pixels in a viewport `height` pixels
    /// tall, and sets the camera's angles, snapped. Right turns as an
    /// orbit drag does; down raises the camera.
    pub fn drag(&mut self, delta: egui::Vec2, height: f32, camera: &mut crate::camera::OrbitCamera) {
        let rate = 180.0 / (height / 2.0).max(1.0);
        if let Some(y) = &mut self.yaw {
            *y -= delta.x * rate;
            camera.yaw = (snapped(*y) - 90.0).to_radians();
        }
        if let Some(p) = &mut self.pitch {
            *p = (*p + delta.y * rate).clamp(-90.0, 90.0);
            // Short of the poles, as an orbit stops.
            camera.pitch = snapped(*p).clamp(-89.4, 89.4).to_radians();
        }
    }
}

/// The faces turned toward a camera of basis `basis` (right, up,
/// forward), farthest first: how much each faces the camera, its corners in
/// screen units (y down) about the cube's centre, its label and view.
pub fn visible_faces(basis: glam::Mat3) -> Vec<(f32, [glam::Vec2; 4], &'static str, View)> {
    let project = |w: Vec3| glam::Vec2::new(w.dot(basis.x_axis), -w.dot(basis.y_axis));
    let mut faces: Vec<_> = FACES
        .iter()
        .filter_map(|&(n, label, view)| {
            let facing = -n.dot(basis.z_axis);
            if facing <= 0.01 {
                return None;
            }
            let (a, b) = if n.z.abs() > 0.5 { (Vec3::X, Vec3::Y) } else if n.x.abs() > 0.5 { (Vec3::Y, Vec3::Z) } else { (Vec3::X, Vec3::Z) };
            Some((facing, [n + a + b, n + a - b, n - a - b, n - a + b].map(project), label, view))
        })
        .collect();
    faces.sort_by(|x, y| x.0.total_cmp(&y.0));
    faces
}

impl App {
    /// Draws the cube and handles a click on it; true if the click was its.
    pub fn view_cube(&mut self, ui: &egui::Ui, viewport: egui::Rect) -> bool {
        if !self.view_cube_shown {
            return false;
        }
        let area = cube_rect(viewport, self.ruler_shown);
        let centre = area.center();
        let scale = area.width() * 0.3;
        let to_screen = |p: glam::Vec2| centre + egui::vec2(p.x, p.y) * scale;
        let faces: Vec<(f32, [egui::Pos2; 4], &str, View)> = visible_faces(self.camera.basis())
            .into_iter()
            .map(|(facing, corners, label, view)| (facing, corners.map(to_screen), label, view))
            .collect();
        let pointer = ui.input(|i| i.pointer.hover_pos());
        let clicked = ui.input(|i| i.pointer.primary_pressed());
        let inside = |p: egui::Pos2, q: &[egui::Pos2; 4]| {
            // Convex: on the same side of every edge.
            let side = |k: usize| {
                let (a, b) = (q[k], q[(k + 1) % 4]);
                (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
            };
            let s: Vec<f32> = (0..4).map(side).collect();
            s.iter().all(|&v| v >= 0.0) || s.iter().all(|&v| v <= 0.0)
        };
        let hovered = pointer.and_then(|p| faces.iter().rev().find(|f| inside(p, &f.1)).map(|f| f.2));
        let painter = ui.painter_at(area);
        for (facing, corners, label, _) in &faces {
            let shade = (90.0 + 80.0 * facing) as u8;
            let fill = if hovered == Some(*label) {
                egui::Color32::from_rgb(110, 150, 210)
            } else {
                egui::Color32::from_rgba_unmultiplied(shade, shade, shade + 8, 200)
            };
            painter.add(egui::Shape::convex_polygon(corners.to_vec(), fill, egui::Stroke::new(1.0, egui::Color32::from_gray(40))));
            if *facing > 0.35 {
                let mid = egui::pos2(corners.iter().map(|c| c.x).sum::<f32>() / 4.0, corners.iter().map(|c| c.y).sum::<f32>() / 4.0);
                painter.text(mid, egui::Align2::CENTER_CENTER, *label, egui::FontId::proportional(10.0), egui::Color32::from_gray(20));
            }
        }
        if clicked && let Some(label) = hovered && let Some(view) = faces.iter().find(|f| f.2 == label).map(|f| f.3) {
            self.camera.set_view(view);
            self.path_tracer.reset();
            return true;
        }
        clicked && pointer.is_some_and(|p| area.contains(p))
    }

    /// Draws the ruler and turns the camera when it is dragged.
    pub fn ruler(&mut self, ui: &egui::Ui, viewport: egui::Rect) {
        if !self.ruler_shown {
            self.ruler_drag = None;
            return;
        }
        let [turn, rise, corner] = ruler_rects(viewport);
        let (pointer, pressed, down, delta) =
            ui.input(|i| (i.pointer.hover_pos(), i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.delta()));
        let iso = self.camera.projection == crate::camera::Projection::Isometric;
        if pressed && !iso && let Some(p) = pointer {
            let on = |r: egui::Rect| r.contains(p);
            if on(turn) || on(rise) || on(corner) {
                let both = on(corner);
                self.ruler_drag = Some(RulerDrag::start(on(turn) || both, on(rise) || both, &self.camera));
            }
        }
        if !down {
            self.ruler_drag = None;
        }
        if let Some(drag) = &mut self.ruler_drag
            && delta != egui::Vec2::ZERO
        {
            drag.drag(delta, viewport.height(), &mut self.camera);
            self.path_tracer.reset();
        }
        let painter = ui.painter_at(viewport);
        let fill = egui::Color32::from_rgba_unmultiplied(90, 90, 90, 140);
        let active = egui::Color32::from_rgba_unmultiplied(110, 150, 210, 170);
        let hover = |r: egui::Rect| pointer.is_some_and(|p| r.contains(p)) && !iso;
        let dragging = self.ruler_drag.map(|d| (d.yaw.is_some(), d.pitch.is_some()));
        let signed = |v: f32| if v > 0.0 { format!("+{v:.0}°") } else { format!("{v:.0}°") };
        for (rect, lit, text, alpha) in [
            (turn, dragging.is_some_and(|d| d.0) || hover(turn), signed(ruler_yaw(self.camera.yaw).floor()), 255),
            (rise, dragging.is_some_and(|d| d.1) || hover(rise), signed(self.camera.pitch.to_degrees().floor()), 220),
            (corner, dragging.is_some_and(|d| d.0 && d.1) || hover(corner), String::new(), 255),
        ] {
            painter.rect_filled(rect.shrink(1.0), 4.0, if lit { active } else { fill });
            if !text.is_empty() {
                painter.text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(10.0), egui::Color32::from_white_alpha(alpha));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::OrbitCamera;

    #[test]
    fn the_ruler_reads_from_the_front_and_drags_in_snapped_steps() {
        let mut c = OrbitCamera::default();
        c.set_view(View::Front);
        assert_eq!(ruler_yaw(c.yaw).round(), 0.0);
        c.set_view(View::Right);
        assert_eq!(ruler_yaw(c.yaw).round().abs(), 90.0);
        c.set_view(View::Back);
        assert_eq!(ruler_yaw(c.yaw).round(), 180.0);
        // Half the viewport's height turns 180°, in 5° steps.
        c.set_view(View::Front);
        c.pitch = 0.3;
        let mut d = RulerDrag::start(true, true, &c);
        d.drag(egui::vec2(-12.0, 0.0), 360.0, &mut c);
        assert!((ruler_yaw(c.yaw) - 10.0).abs() < 1e-3, "{}", ruler_yaw(c.yaw));
        assert!((c.pitch.to_degrees() - 15.0).abs() < 1e-3, "starts snapped: {}", c.pitch.to_degrees());
        d.drag(egui::vec2(0.0, 1000.0), 360.0, &mut c);
        assert!(c.pitch.to_degrees() < 90.0 && c.pitch.to_degrees() > 89.0);
        // A strip alone leaves the other angle.
        let before = c.pitch;
        let mut turn = RulerDrag::start(true, false, &c);
        turn.drag(egui::vec2(30.0, 30.0), 360.0, &mut c);
        assert_eq!(c.pitch, before);
    }

    #[test]
    fn each_view_faces_its_own_face() {
        for (view, label) in [(View::Front, "Front"), (View::Back, "Back"), (View::Left, "Left"), (View::Right, "Right")] {
            let mut c = OrbitCamera::default();
            c.set_view(view);
            let faces = visible_faces(c.basis());
            let (facing, corners, front, _) = faces.last().copied().expect("a face");
            assert_eq!(front, label);
            assert!(facing > 0.99);
            // Square on, the face fills the middle.
            let (lo, hi) = corners.iter().fold((glam::Vec2::MAX, glam::Vec2::MIN), |(a, b), p| (a.min(*p), b.max(*p)));
            assert!(lo.x < 0.0 && hi.x > 0.0 && lo.y < 0.0 && hi.y > 0.0);
        }
        let mut c = OrbitCamera::default();
        c.set_view(View::Top);
        assert_eq!(visible_faces(c.basis()).last().unwrap().2, "Top");
        // The default view shows three faces.
        assert_eq!(visible_faces(OrbitCamera::default().basis()).len(), 3);
    }
}
