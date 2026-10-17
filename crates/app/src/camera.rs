//! Orbit camera.

use glam::{Mat3, Mat4, Vec3};
pub use voxel_core::saved_camera::Projection;
// glam 0.34 groups projections by graphics API rather than by handedness.
// wgpu wants NDC with Z in 0..1 and Y up, which is the `directx` variant --
// `vulkan` flips Y, and `opengl` maps Z to -1..1 and would clip half the
// scene away. Metal shares the DirectX convention despite the module name.
use glam::camera::rh::{
    proj::directx::{orthographic, perspective},
    view::look_at_mat4,
};

/// Stop just short of vertical: at exactly ±90° the up vector and the view
/// direction become parallel and look_at degenerates.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

/// The fixed views of the editor's view menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Front,
    Back,
    Left,
    Right,
    Top,
    Bottom,
    /// Orthographic, from the front-right corner, 35.26° up: the angle at
    /// which the three axes look equally long.
    Iso,
}

impl View {
    pub const ALL: [(View, &'static str); 7] = [
        (View::Front, "front"),
        (View::Back, "back"),
        (View::Left, "left"),
        (View::Right, "right"),
        (View::Top, "top"),
        (View::Bottom, "bottom"),
        (View::Iso, "iso"),
    ];

    pub fn parse(s: &str) -> Option<View> {
        Self::ALL.iter().find(|(_, n)| *n == s).map(|(v, _)| *v)
    }
}

/// A camera that orbits a target point.
///
/// Z is up, matching the `.vox` coordinate system, so models load the way up
/// the editor shows them.
pub struct OrbitCamera {
    pub target: Vec3,
    pub distance: f32,
    /// Rotation about the up axis, radians.
    pub yaw: f32,
    /// Elevation, radians, clamped short of the poles.
    pub pitch: f32,
    pub fov_y: f32,
    /// the reference editor's four modes. In the parallel ones the visible height is
    /// what the perspective view shows at the target, so switching keeps
    /// the subject the same size. Free turns about the eye instead of the
    /// target and flies with the keys; isometric keeps its pitch and turns
    /// a quarter at a time.
    pub projection: Projection,
    /// Drag gathered toward the next isometric quarter turn.
    iso_turn: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 100.0,
            yaw: 0.6,
            pitch: 0.5,
            fov_y: 45f32.to_radians(),
            projection: Projection::Perspective,
            iso_turn: 0.0,
        }
    }
}

impl OrbitCamera {
    /// Frames a world-space box, as produced by a meshed scene.
    pub fn frame_world_bounds(&mut self, min: Vec3, max: Vec3) {
        self.target = (min + max) * 0.5;
        let radius = (max - min).length() * 0.5;
        self.distance = (radius.max(1.0) / (self.fov_y * 0.5).tan()) * 1.1;
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * cy, cp * sy, sp) * self.distance
    }

    /// Parallel projection: orthographic or isometric.
    pub fn ortho(&self) -> bool {
        self.projection.is_parallel()
    }

    /// The isometric pitch: the three axes equally foreshortened.
    pub fn iso_pitch() -> f32 {
        (1.0 / 2f32.sqrt()).atan()
    }

    /// Switches mode. Isometric snaps to its pitch and the nearest diagonal.
    pub fn set_projection(&mut self, projection: Projection) {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
        self.projection = projection;
        self.iso_turn = 0.0;
        if projection == Projection::Isometric {
            self.pitch = Self::iso_pitch();
            self.yaw = ((self.yaw - FRAC_PI_4) / FRAC_PI_2).round() * FRAC_PI_2 + FRAC_PI_4;
        }
    }

    /// A mouse drag: orbiting the target, or, in free mode, looking around
    /// from the eye; isometric turns a quarter for each half radian dragged.
    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        match self.projection {
            Projection::Isometric => {
                use std::f32::consts::FRAC_PI_2;
                self.iso_turn += delta_yaw;
                while self.iso_turn.abs() >= 0.5 {
                    let step = self.iso_turn.signum();
                    self.yaw -= step * FRAC_PI_2;
                    self.iso_turn -= step * 0.5;
                }
            }
            Projection::Free => {
                // The eye stays; the target swings round it. Dragging turns
                // the view the way the cursor goes, as looking does.
                let eye = self.eye();
                self.yaw += delta_yaw;
                self.pitch = (self.pitch + delta_pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
                self.target = eye - (self.eye() - self.target);
            }
            _ => {
                self.yaw -= delta_yaw;
                self.pitch = (self.pitch + delta_pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);
            }
        }
    }

    /// Free mode's keys: `forward`, `right` and `up` in units of distance
    /// to move, along the view (W/S), across it (A/D) and the world's up
    /// (E/Q). The eye and target move together.
    pub fn fly(&mut self, forward: f32, right: f32, up: f32) {
        let b = self.basis();
        self.target += b.z_axis * forward + b.x_axis * right + Vec3::Z * up;
    }

    /// Turns to a fixed view, keeping target and distance. Every view but
    /// `Iso` keeps the current projection.
    pub fn set_view(&mut self, view: View) {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};
        // Yaw 0 looks from +x; -90° from -y, the editor's front.
        let (yaw, pitch) = match view {
            View::Front => (-FRAC_PI_2, 0.0),
            View::Back => (FRAC_PI_2, 0.0),
            View::Left => (PI, 0.0),
            View::Right => (0.0, 0.0),
            View::Top => (-FRAC_PI_2, PITCH_LIMIT),
            View::Bottom => (-FRAC_PI_2, -PITCH_LIMIT),
            View::Iso => {
                self.projection = Projection::Isometric;
                self.iso_turn = 0.0;
                (-FRAC_PI_4, Self::iso_pitch())
            }
        };
        self.yaw = yaw;
        self.pitch = pitch;
    }

    /// Right, up and forward, as matrix columns.
    pub fn basis(&self) -> Mat3 {
        let forward = (self.target - self.eye()).normalize_or(Vec3::Y);
        let right = forward.cross(Vec3::Z).normalize_or(Vec3::X);
        let up = right.cross(forward);
        Mat3::from_cols(right, up, forward)
    }

    /// Half the visible height at the target, in world units: the
    /// orthographic extent, and the scale for panning.
    pub fn half_height(&self) -> f32 {
        self.distance * (self.fov_y * 0.5).tan()
    }

    /// Slides the target in the view plane, by pixels of a viewport
    /// `viewport_height` pixels tall, so the scene follows the cursor.
    pub fn pan(&mut self, dx: f32, dy: f32, viewport_height: f32) {
        let per_pixel = 2.0 * self.half_height() / viewport_height.max(1.0);
        let b = self.basis();
        self.target += (-b.x_axis * dx + b.y_axis * dy) * per_pixel;
    }

    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(1.0, 10_000.0);
    }

    /// Combined view-projection matrix.
    ///
    /// See the import comment for why this is the `directx` projection.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let aspect = aspect.max(0.01);
        let proj = if self.ortho() {
            let h = self.half_height();
            // Near behind the eye: in a parallel view the eye's place along
            // the axis is arbitrary, and nothing between it and the target
            // should be clipped.
            orthographic(-h * aspect, h * aspect, -h, h, -50_000.0, 50_000.0)
        } else {
            perspective(self.fov_y, aspect, 0.1, 100_000.0)
        };
        let view = look_at_mat4(self.eye(), self.target, Vec3::Z);
        proj * view
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_never_reaches_the_pole() {
        let mut c = OrbitCamera::default();
        c.orbit(0.0, 100.0);
        assert!(c.pitch < std::f32::consts::FRAC_PI_2);
        c.orbit(0.0, -200.0);
        assert!(c.pitch > -std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn framing_centres_and_fits_the_box() {
        let mut big = OrbitCamera::default();
        big.frame_world_bounds(Vec3::ZERO, Vec3::splat(100.0));
        let mut small = OrbitCamera::default();
        small.frame_world_bounds(Vec3::splat(40.0), Vec3::splat(60.0));

        assert_eq!(small.target, Vec3::splat(50.0));
        assert!(
            small.distance < big.distance,
            "a smaller box should pull the camera in"
        );
    }

    #[test]
    fn the_front_view_looks_along_plus_y() {
        let mut c = OrbitCamera::default();
        c.set_view(View::Front);
        let forward = c.basis().z_axis;
        assert!(forward.y > 0.999, "{forward:?}");
        c.set_view(View::Top);
        assert!(c.basis().z_axis.z < -0.99, "top looks down");
    }

    #[test]
    fn iso_is_orthographic_with_equal_axes() {
        let mut c = OrbitCamera::default();
        c.set_view(View::Iso);
        assert!(c.ortho());
        // Equal foreshortening: the three axes project to equal lengths.
        let b = c.basis();
        let len = |a: Vec3| Vec3::new(a.dot(b.x_axis), a.dot(b.y_axis), 0.0).length();
        let (x, y, z) = (len(Vec3::X), len(Vec3::Y), len(Vec3::Z));
        assert!((x - y).abs() < 1e-3 && (x - z).abs() < 0.02, "{x} {y} {z}");
    }

    #[test]
    fn panning_follows_the_cursor() {
        let mut c = OrbitCamera::default();
        c.set_view(View::Front);
        let before = c.target;
        c.pan(10.0, 0.0, 100.0);
        // Dragging right drags the scene right: the camera moves left (-x).
        assert!(c.target.x < before.x);
        assert!((c.target.z - before.z).abs() < 1e-4);
    }

    #[test]
    fn orthographic_projects_parallel_lines_parallel() {
        let mut c = OrbitCamera::default();
        c.set_view(View::Front);
        c.projection = Projection::Orthographic;
        let vp = c.view_proj(1.0);
        let ndc = |p: Vec3| {
            let v = vp * p.extend(1.0);
            v.truncate() / v.w
        };
        // The same point near and far keeps its screen position.
        let near = ndc(Vec3::new(5.0, -20.0, 3.0));
        let far = ndc(Vec3::new(5.0, 20.0, 3.0));
        assert!((near.x - far.x).abs() < 1e-5 && (near.y - far.y).abs() < 1e-5);
    }

    #[test]
    fn free_looks_from_the_eye_and_flies() {
        let mut c = OrbitCamera::default();
        c.set_projection(Projection::Free);
        let eye = c.eye();
        c.orbit(0.3, 0.1);
        assert!((c.eye() - eye).length() < 1e-3, "the eye stays put");
        let forward = c.basis().z_axis;
        c.fly(2.0, 0.0, 0.0);
        assert!((c.eye() - (eye + forward * 2.0)).length() < 1e-3);
    }

    #[test]
    fn isometric_turns_in_quarters_at_its_pitch() {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
        let mut c = OrbitCamera::default();
        c.set_projection(Projection::Isometric);
        assert!(c.ortho());
        assert!((c.pitch - OrbitCamera::iso_pitch()).abs() < 1e-6);
        let quarter = |y: f32| ((y - FRAC_PI_4) / FRAC_PI_2 - ((y - FRAC_PI_4) / FRAC_PI_2).round()).abs() < 1e-4;
        assert!(quarter(c.yaw));
        let before = c.yaw;
        c.orbit(0.2, 0.3);
        assert_eq!(c.yaw, before, "small drags gather");
        c.orbit(0.35, 0.0);
        assert!((c.yaw - (before - FRAC_PI_2)).abs() < 1e-5);
        assert!((c.pitch - OrbitCamera::iso_pitch()).abs() < 1e-6, "pitch held");
    }
}
