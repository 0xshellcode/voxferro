//! Saved cameras: the editor's `rCAM` slots, as views this editor can use.
//!
//! What the keys mean, read from `the reference writer` in the editor (`SaveCamera`,
//! `LoadCamera`, `SetAngle`, `SetMode`; macOS a recent version and Windows
//! a recent version):
//!
//! - `_mode`: `pers`, `free`, `orth` or `iso` (index 0-3 on Windows).
//! - `_focus`: the point the camera orbits, in world coordinates.
//! - `_angle`: pitch, yaw, roll in degrees -- the editor's own camera panel
//!   labels them so. Pitch is clamped to ±90 and yaw kept in 0..360; `iso`
//!   forces the pitch to ±35.26°.
//! - `_radius`: the distance from the focus to the eye.
//! - `_frustum`: `tan(fov / 2)` in perspective; in the parallel modes the
//!   half height of the view, which `SetMode` derives as
//!   `radius * tan(fov / 2)` -- the same rule this editor uses.
//! - `_fov`: the vertical field of view in degrees.
//!
//! The eye sits at `focus + radius * B * (sin yaw cos pitch, -sin pitch,
//! cos yaw cos pitch)`, `B` being a fixed base change the binary keeps in
//! the camera. Rather than reading `B` out of an initialiser, it was
//! measured in the editor (macOS a recent version) with a model marked on +X, +Y
//! and +Z, and then checked by loading slots this editor wrote. Its camera
//! panel shows the file's yaw negated -- a slot saved as `0 90 0` shows
//! "Yaw -90", `-35.26 315 0` shows 45 -- while pitch matches. In file
//! terms: pitch -20, yaw 0 looks from -Y and from above; yaw 90 looks from
//! +X. So the direction from the focus to the eye is
//! `(sin yaw cos pitch, -cos yaw cos pitch, -sin pitch)`.
//!
//! This editor's orbit camera puts the eye at `(cos p cos y, cos p sin y,
//! sin p)` from its target, so `p = -pitch` and `y = yaw - 90°`.

use glam::Vec3;
use vox_format::{Camera, Dict};

/// The editor's four camera modes, as `_mode` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Projection {
    /// Orbiting a focus, in perspective.
    #[default]
    Perspective,
    /// In perspective, turning about the eye and flying with the keys.
    Free,
    /// Orbiting, parallel projection.
    Orthographic,
    /// Parallel, at the isometric pitch, turning in quarter turns.
    Isometric,
}

impl Projection {
    pub const ALL: [(Projection, &'static str); 4] = [
        (Projection::Perspective, "pers"),
        (Projection::Free, "free"),
        (Projection::Orthographic, "orth"),
        (Projection::Isometric, "iso"),
    ];

    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(p, _)| *p == self).map_or("pers", |(_, n)| n)
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, n)| *n == name).map(|(p, _)| *p)
    }

    /// Orthographic or isometric.
    pub fn is_parallel(self) -> bool {
        matches!(self, Projection::Orthographic | Projection::Isometric)
    }
}

/// The editor keeps ten camera slots.
pub const SLOTS: usize = 10;

/// A view in this editor's terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SavedView {
    pub projection: Projection,
    pub target: Vec3,
    /// Radians, this editor's orbit convention.
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    /// Vertical field of view, radians.
    pub fov_y: f32,
}

impl SavedView {
    /// The view a camera slot holds, or `None` for a slot never saved: the
    /// editor fills unused slots with a zero radius at the origin.
    pub fn from_camera(camera: &Camera) -> Option<Self> {
        let a = &camera.attributes;
        let floats = |key: &str| -> Vec<f32> {
            a.get(key).map(|v| v.split_whitespace().filter_map(|n| n.parse().ok()).collect()).unwrap_or_default()
        };
        let one = |key: &str| floats(key).first().copied();
        let radius = one("_radius")?;
        if radius <= 0.0 {
            return None;
        }
        let focus = floats("_focus");
        let angle = floats("_angle");
        let (pitch, yaw) = (*angle.first().unwrap_or(&0.0), *angle.get(1).unwrap_or(&0.0));
        let fov_deg = one("_fov").filter(|f| *f > 0.0).unwrap_or(45.0);
        let fov_y = fov_deg.to_radians();
        let projection = Projection::parse(camera.mode()).unwrap_or_default();
        let ortho = projection.is_parallel();
        // In the parallel modes the scale is the stored half height: put
        // the eye where this editor's `distance * tan(fov / 2)` gives it.
        let distance = match one("_frustum") {
            Some(h) if ortho && h > 0.0 => h / (fov_y * 0.5).tan(),
            _ => radius,
        };
        Some(Self {
            projection,
            target: Vec3::new(
                *focus.first().unwrap_or(&0.0),
                *focus.get(1).unwrap_or(&0.0),
                *focus.get(2).unwrap_or(&0.0),
            ),
            yaw: (yaw - 90.0).to_radians(),
            pitch: -pitch.clamp(-90.0, 90.0).to_radians(),
            distance,
            fov_y,
        })
    }

    /// The `rCAM` entry for slot `id`, keys in the order the editor writes.
    pub fn to_camera(&self, id: i32) -> Camera {
        let pitch = -self.pitch.to_degrees();
        let yaw = (self.yaw.to_degrees() + 90.0).rem_euclid(360.0);
        let tan = (self.fov_y * 0.5).tan();
        let mut attributes = Dict::default();
        let ortho = self.projection.is_parallel();
        attributes.set("_mode", self.projection.name());
        attributes.set("_focus", format!("{} {} {}", g(self.target.x), g(self.target.y), g(self.target.z)));
        attributes.set("_angle", format!("{} {} 0", g(pitch), g(yaw)));
        attributes.set("_radius", g(self.distance));
        attributes.set("_frustum", g(if ortho { self.distance * tan } else { tan }));
        attributes.set("_fov", g(self.fov_y.to_degrees()));
        Camera { id, attributes }
    }

    /// Direction from the target to the eye.
    pub fn back(&self) -> Vec3 {
        Vec3::new(self.pitch.cos() * self.yaw.cos(), self.pitch.cos() * self.yaw.sin(), self.pitch.sin())
    }
}

/// Like C's `%g` closely enough: up to six significant digits, no
/// trailing zeros.
fn g(v: f32) -> String {
    let v = if v.abs() < 1e-6 { 0.0 } else { v };
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".to_owned() } else { s.to_owned() }
}

/// The ten slots of a file: its cameras by id, with `None` for slots that
/// hold no view.
pub fn slots(cameras: &[Camera]) -> [Option<SavedView>; SLOTS] {
    let mut out = [None; SLOTS];
    for c in cameras {
        if let Some(slot) = usize::try_from(c.id).ok().filter(|i| *i < SLOTS) {
            out[slot] = SavedView::from_camera(c);
        }
    }
    out
}

/// `cameras` with slot `id` replaced by `view`, keeping every other entry
/// (and the editor's unused defaults) as they were.
pub fn with_slot(cameras: &[Camera], id: i32, view: &SavedView) -> Vec<Camera> {
    let mut out = cameras.to_vec();
    let camera = view.to_camera(id);
    match out.iter_mut().find(|c| c.id == id) {
        Some(slot) => *slot = camera,
        None => {
            out.push(camera);
            out.sort_by_key(|c| c.id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera(mode: &str, focus: &str, angle: &str, radius: &str) -> Camera {
        let mut attributes = Dict::default();
        attributes.set("_mode", mode);
        attributes.set("_focus", focus);
        attributes.set("_angle", angle);
        attributes.set("_radius", radius);
        attributes.set("_frustum", "0.414214");
        attributes.set("_fov", "45");
        Camera { id: 0, attributes }
    }

    #[test]
    fn the_measured_views_come_out_where_the_editor_shows_them() {
        // Pitch -20, yaw 0: from -Y, above.
        let v = SavedView::from_camera(&camera("pers", "0 0 8", "-20 0 0", "47.7")).unwrap();
        let b = v.back();
        assert!(b.y < -0.9 && b.z > 0.3 && b.x.abs() < 1e-5, "{b:?}");
        assert_eq!(v.target, Vec3::new(0.0, 0.0, 8.0));
        // Yaw 90 (the editor's panel shows -90): from +X.
        let v = SavedView::from_camera(&camera("pers", "0 0 8", "-20 90 0", "47.7")).unwrap();
        assert!(v.back().x > 0.9, "{:?}", v.back());
    }

    #[test]
    fn unused_slots_are_empty_and_writing_reads_back() {
        assert!(SavedView::from_camera(&camera("pers", "0 0 0", "0 0 0", "0")).is_none());
        let view = SavedView {
            projection: Projection::Perspective,
            target: Vec3::new(1.0, -2.5, 8.0),
            yaw: (-135f32).to_radians(),
            pitch: 30f32.to_radians(),
            distance: 60.0,
            fov_y: 45f32.to_radians(),
        };
        let written = view.to_camera(3);
        assert_eq!(written.attributes.get("_angle"), Some("-30 315 0"));
        assert_eq!(written.attributes.get("_frustum"), Some("0.4142"));
        let back = SavedView::from_camera(&written).unwrap();
        assert!((back.yaw.sin() - view.yaw.sin()).abs() < 1e-4 && (back.yaw.cos() - view.yaw.cos()).abs() < 1e-4);
        assert!((back.pitch - view.pitch).abs() < 1e-4);
        assert_eq!(back.target, view.target);
    }

    #[test]
    fn orthographic_slots_keep_their_scale() {
        let mut c = camera("orth", "0 0 0", "-35.26 45 0", "100");
        c.attributes.set("_frustum", "20");
        let v = SavedView::from_camera(&c).unwrap();
        assert!(v.projection.is_parallel());
        assert!((v.distance * (v.fov_y * 0.5).tan() - 20.0).abs() < 1e-3);
        let slots = slots(&with_slot(&[], 2, &v));
        assert!(slots[2].is_some() && slots[0].is_none());
    }
}
