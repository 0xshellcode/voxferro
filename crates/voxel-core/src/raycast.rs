//! Walking a ray through the voxel grid.

use crate::{Face, Volume};
use glam::{IVec3, Vec3};

/// What a ray hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// The filled voxel that was hit.
    pub voxel: IVec3,
    /// The face it entered through. Placing a new voxel means stepping one
    /// cell along this face's normal.
    pub face: Face,
}

impl Hit {
    /// The empty cell adjacent to the hit face, where a new voxel goes.
    pub fn adjacent(&self) -> IVec3 {
        let o = self.face.offset();
        self.voxel + IVec3::new(o[0], o[1], o[2])
    }
}

/// Marches a ray through the grid and returns the first filled voxel.
///
/// Amanatides-Woo DDA: step cell by cell along whichever axis reaches its next
/// boundary first. Every cell the ray touches is visited exactly once, in
/// order, so the first filled one found is genuinely the nearest — which
/// sampling at fixed intervals cannot guarantee, since it can tunnel through
/// a thin wall.
///
/// Picking on the GPU, encoding the face into a framebuffer, would scale
/// better; this needs no GPU round trip and is exact.
pub fn raycast(volume: &Volume, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<Hit> {
    let dir = direction.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }

    let mut cell = origin.floor().as_ivec3();
    let step = IVec3::new(
        if dir.x > 0.0 { 1 } else { -1 },
        if dir.y > 0.0 { 1 } else { -1 },
        if dir.z > 0.0 { 1 } else { -1 },
    );

    // Distance along the ray between successive boundaries of each axis.
    // An axis the ray is parallel to never crosses one, hence infinity.
    let delta = Vec3::new(
        if dir.x != 0.0 { (1.0 / dir.x).abs() } else { f32::INFINITY },
        if dir.y != 0.0 { (1.0 / dir.y).abs() } else { f32::INFINITY },
        if dir.z != 0.0 { (1.0 / dir.z).abs() } else { f32::INFINITY },
    );

    // Distance to the first boundary on each axis.
    let next_boundary = |o: f32, d: f32, c: i32| -> f32 {
        if d > 0.0 {
            (c as f32 + 1.0 - o).abs()
        } else {
            (o - c as f32).abs()
        }
    };
    let mut t_max = Vec3::new(
        if dir.x != 0.0 { next_boundary(origin.x, dir.x, cell.x) * delta.x } else { f32::INFINITY },
        if dir.y != 0.0 { next_boundary(origin.y, dir.y, cell.y) * delta.y } else { f32::INFINITY },
        if dir.z != 0.0 { next_boundary(origin.z, dir.z, cell.z) * delta.z } else { f32::INFINITY },
    );

    let [sx, sy, sz] = volume.size();
    let mut face = Face::NegX;
    let mut travelled = 0.0;

    // A step budget as well as a distance limit: a ray starting far outside a
    // large grid should give up rather than iterate for a very long time.
    let max_steps = (sx + sy + sz) as usize * 3 + 32;

    for _ in 0..max_steps {
        if travelled > max_distance {
            return None;
        }

        // Only test cells inside the grid; the ray may start outside it.
        let inside = cell.x >= 0
            && cell.y >= 0
            && cell.z >= 0
            && (cell.x as u32) < sx
            && (cell.y as u32) < sy
            && (cell.z as u32) < sz;

        if inside && volume.get_i(cell.x, cell.y, cell.z) != 0 {
            return Some(Hit { voxel: cell, face });
        }

        // Advance along whichever axis hits its next boundary soonest, and
        // remember which one: that is the face the ray enters through.
        if t_max.x < t_max.y && t_max.x < t_max.z {
            travelled = t_max.x;
            cell.x += step.x;
            t_max.x += delta.x;
            face = if step.x > 0 { Face::NegX } else { Face::PosX };
        } else if t_max.y < t_max.z {
            travelled = t_max.y;
            cell.y += step.y;
            t_max.y += delta.y;
            face = if step.y > 0 { Face::NegY } else { Face::PosY };
        } else {
            travelled = t_max.z;
            cell.z += step.z;
            t_max.z += delta.z;
            face = if step.z > 0 { Face::NegZ } else { Face::PosZ };
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume_with(points: &[(u32, u32, u32)]) -> Volume {
        let mut v = Volume::new([16, 16, 16]);
        for &(x, y, z) in points {
            v.set(x, y, z, 1);
        }
        v
    }

    #[test]
    fn hits_the_nearest_voxel_not_a_later_one() {
        let v = volume_with(&[(5, 8, 8), (10, 8, 8)]);
        let hit = raycast(&v, Vec3::new(0.5, 8.5, 8.5), Vec3::X, 100.0).unwrap();
        assert_eq!(hit.voxel, IVec3::new(5, 8, 8));
        assert_eq!(hit.face, Face::NegX, "entered from the -X side");
    }

    #[test]
    fn misses_return_none() {
        let v = volume_with(&[(5, 8, 8)]);
        assert!(raycast(&v, Vec3::new(0.5, 0.5, 0.5), Vec3::X, 100.0).is_none());
    }

    #[test]
    fn does_not_tunnel_through_a_thin_wall() {
        // A diagonal ray must still notice a one-voxel-thick wall. Sampling
        // at fixed steps is exactly what fails here.
        let mut v = Volume::new([16, 16, 16]);
        for y in 0..16 {
            for z in 0..16 {
                v.set(8, y, z, 1);
            }
        }
        let hit = raycast(
            &v,
            Vec3::new(0.5, 0.5, 0.5),
            Vec3::new(1.0, 0.3, 0.2),
            100.0,
        );
        assert_eq!(hit.map(|h| h.voxel.x), Some(8));
    }

    #[test]
    fn adjacent_cell_is_outside_the_hit_voxel() {
        let v = volume_with(&[(5, 8, 8)]);
        let hit = raycast(&v, Vec3::new(0.5, 8.5, 8.5), Vec3::X, 100.0).unwrap();
        assert_eq!(hit.adjacent(), IVec3::new(4, 8, 8));
    }

    #[test]
    fn respects_the_distance_limit() {
        let v = volume_with(&[(15, 8, 8)]);
        assert!(raycast(&v, Vec3::new(0.5, 8.5, 8.5), Vec3::X, 3.0).is_none());
    }

    #[test]
    fn finds_a_voxel_when_starting_outside_the_grid() {
        let v = volume_with(&[(0, 8, 8)]);
        let hit = raycast(&v, Vec3::new(-10.0, 8.5, 8.5), Vec3::X, 100.0).unwrap();
        assert_eq!(hit.voxel, IVec3::new(0, 8, 8));
    }
}
