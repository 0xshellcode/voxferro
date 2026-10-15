//! Orbit camera.

use glam::{Mat4, Vec3};
// glam 0.34 groups projections by graphics API rather than by handedness.
// wgpu wants NDC with Z in 0..1 and Y up, which is the `directx` variant --
// `vulkan` flips Y, and `opengl` maps Z to -1..1 and would clip half the
// scene away. Metal shares the DirectX convention despite the module name.
use glam::camera::rh::{proj::directx::perspective, view::look_at_mat4};

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
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target: Vec3::ZERO,
            distance: 100.0,
            yaw: 0.6,
            pitch: 0.5,
            fov_y: 45f32.to_radians(),
        }
    }
}

impl OrbitCamera {
    /// Frames an axis-aligned box so it fills the view.
    ///
    /// `min`/`max` are inclusive voxel coordinates, so the box spans
    /// `max - min + 1` voxels on each axis.
    pub fn frame_bounds(&mut self, min: [u32; 3], max: [u32; 3]) {
        let lo = Vec3::new(min[0] as f32, min[1] as f32, min[2] as f32);
        let hi = Vec3::new(max[0] as f32 + 1.0, max[1] as f32 + 1.0, max[2] as f32 + 1.0);
        self.target = (lo + hi) * 0.5;
        // Fit the box's bounding sphere into the vertical field of view, with
        // a small margin so it does not touch the edges.
        let radius = (hi - lo).length() * 0.5;
        self.distance = (radius / (self.fov_y * 0.5).tan()) * 1.1;
    }

    /// Frames a whole volume by its declared size.
    pub fn frame_volume(&mut self, size: [u32; 3]) {
        if size.iter().any(|&s| s == 0) {
            return;
        }
        self.frame_bounds([0, 0, 0], [size[0] - 1, size[1] - 1, size[2] - 1]);
    }

    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * cy, cp * sy, sp) * self.distance
    }

    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.yaw -= delta_yaw;
        // Stop just short of vertical: at exactly ±90° the up vector and the
        // view direction become parallel and look_at degenerates.
        const LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;
        self.pitch = (self.pitch + delta_pitch).clamp(-LIMIT, LIMIT);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(1.0, 10_000.0);
    }

    /// Combined view-projection matrix.
    ///
    /// See the import comment for why this is the `directx` projection.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let proj = perspective(self.fov_y, aspect.max(0.01), 0.1, 100_000.0);
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
    fn framing_centres_the_volume() {
        let mut c = OrbitCamera::default();
        c.frame_volume([10, 20, 30]);
        assert_eq!(c.target, Vec3::new(5.0, 10.0, 15.0));
        assert!(c.distance > 20.0, "should back off far enough to see it");
    }

    #[test]
    fn framing_bounds_is_tighter_than_the_whole_volume() {
        let mut whole = OrbitCamera::default();
        whole.frame_volume([100, 100, 100]);
        let mut part = OrbitCamera::default();
        part.frame_bounds([40, 40, 40], [59, 59, 59]);
        assert!(
            part.distance < whole.distance,
            "a small occupied box should pull the camera in"
        );
        assert_eq!(part.target, Vec3::new(50.0, 50.0, 50.0));
    }
}
