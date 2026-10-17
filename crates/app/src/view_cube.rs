//! The view cube, in the viewport's corner: a cube turned with the camera,
//! its faces named; a click on one goes to that view. the reference editor shows one
//! too (`lower.ui`'s "Show View Cube"); `view cube on|off`.

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

/// Where the cube sits in a viewport.
pub fn cube_rect(viewport: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_size(viewport.right_bottom() - egui::vec2(96.0, 96.0), egui::vec2(84.0, 84.0))
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
        let area = cube_rect(viewport);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::OrbitCamera;

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
