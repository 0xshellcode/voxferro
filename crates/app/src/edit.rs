//! Turning a click into a voxel edit.

use glam::{Mat4, Vec2, Vec3, Vec4, Vec4Swizzles};

/// A ray in world space.
#[derive(Debug, Clone, Copy)]
pub struct Ray {
    pub origin: Vec3,
    pub direction: Vec3,
}

/// Builds a world-space ray through a point on the viewport.
///
/// `pixel` is in egui's screen space, where Y grows downwards, while NDC has Y
/// up and — on wgpu — Z running 0 (near) to 1 (far).
pub fn ray_through_pixel(view_proj: Mat4, viewport: Vec2, pixel: Vec2) -> Ray {
    let ndc = Vec2::new(
        (pixel.x / viewport.x) * 2.0 - 1.0,
        1.0 - (pixel.y / viewport.y) * 2.0,
    );

    let inverse = view_proj.inverse();
    let unproject = |z: f32| -> Vec3 {
        let p = inverse * Vec4::new(ndc.x, ndc.y, z, 1.0);
        // Perspective divide; w is zero only for degenerate matrices.
        p.xyz() / p.w.max(f32::EPSILON)
    };

    let near = unproject(0.0);
    let far = unproject(1.0);
    Ray {
        origin: near,
        direction: (far - near).normalize_or_zero(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_proj() -> Mat4 {
        let proj = glam::camera::rh::proj::directx::perspective(
            45f32.to_radians(),
            1.0,
            0.1,
            1000.0,
        );
        let view = glam::camera::rh::view::look_at_mat4(
            Vec3::new(0.0, -50.0, 0.0),
            Vec3::ZERO,
            Vec3::Z,
        );
        proj * view
    }

    #[test]
    fn centre_pixel_aims_at_the_target() {
        let ray = ray_through_pixel(view_proj(), Vec2::splat(100.0), Vec2::splat(50.0));
        // Camera sits on -Y looking at the origin, so the centre ray runs +Y.
        assert!(ray.direction.y > 0.95, "got {:?}", ray.direction);
    }

    #[test]
    fn pixels_left_and_right_diverge_horizontally() {
        let vp = view_proj();
        let size = Vec2::splat(100.0);
        let left = ray_through_pixel(vp, size, Vec2::new(10.0, 50.0));
        let right = ray_through_pixel(vp, size, Vec2::new(90.0, 50.0));
        assert!(left.direction.x < right.direction.x);
    }

    #[test]
    fn a_click_adds_a_voxel_and_undo_removes_it() {
        use voxel_core::{Document, History, Instance, Scene, Transform, Volume};

        let mut v = Volume::new([8, 8, 8]);
        // Fill the slab the centre ray must cross.
        for x in 0..8 {
            for z in 0..8 {
                v.set(x, 4, z, 1);
            }
        }
        let mut doc = Document {
            scene: Scene {
                models: vec![v],
                instances: vec![Instance {
                    model_index: 0,
                    transform: Transform::default(),
                    layer: -1,
                    ..Default::default()
                }],
                nodes: Vec::new(),
                frame: 0,
            },
            ..Default::default()
        };

        let ray = ray_through_pixel(view_proj(), Vec2::splat(100.0), Vec2::splat(50.0));
        let pick = doc
            .scene
            .pick(ray.origin, ray.direction, 1000.0, |_| true)
            .expect("centre ray hits the slab");

        // Adding places a voxel in the empty cell on the hit face.
        let target = pick.hit.adjacent();
        let at = |d: &Document| d.scene.models[0].get_i(target.x, target.y, target.z);
        assert_eq!(at(&doc), 0, "target starts empty");

        let mut history = History::default();
        history.set_voxels(&mut doc, 0, [(target, 42)]);
        assert_eq!(at(&doc), 42);

        assert!(history.undo(&mut doc));
        assert_eq!(at(&doc), 0, "undo should clear the added voxel");
    }
}
