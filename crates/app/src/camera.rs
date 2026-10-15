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
}
