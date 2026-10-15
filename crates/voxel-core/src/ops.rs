//! Whole-model operations: rotate, mirror, crop, resize.
//!
//! Each returns a new volume; the app records the swap as one
//! `Command::ReplaceModel`, since a change of size cannot be expressed as
//! cell writes.

use crate::Volume;

/// Rotates a quarter turn counter-clockwise about `axis` (0 x, 1 y, 2 z),
/// looking down the axis from its positive end. The other two sizes swap.
pub fn rotate(volume: &Volume, axis: usize) -> Volume {
    let [sx, sy, sz] = volume.size();
    let size = match axis {
        0 => [sx, sz, sy],
        1 => [sz, sy, sx],
        _ => [sy, sx, sz],
    };
    let mut out = Volume::new(size);
    for z in 0..sz {
        for y in 0..sy {
            for x in 0..sx {
                let v = volume.get(x, y, z);
                if v == 0 {
                    continue;
                }
                // +90 degrees: about x (y, z) -> (-z, y); about y
                // (z, x) -> (-x, z); about z (x, y) -> (-y, x). Each negated
                // coordinate is re-based so it stays inside the new box.
                let (nx, ny, nz) = match axis {
                    0 => (x, sz - 1 - z, y),
                    1 => (z, y, sx - 1 - x),
                    _ => (sy - 1 - y, x, z),
                };
                out.set(nx, ny, nz, v);
            }
        }
    }
    out
}

/// Flips along `axis`.
pub fn mirror(volume: &Volume, axis: usize) -> Volume {
    let size = volume.size();
    let mut out = Volume::new(size);
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                let mut p = [x, y, z];
                p[axis] = size[axis] - 1 - p[axis];
                out.set(p[0], p[1], p[2], volume.get(x, y, z));
            }
        }
    }
    out
}

/// Shrinks the box to the filled voxels. `None` when the model is empty,
/// which has nothing to crop to.
pub fn crop(volume: &Volume) -> Option<Volume> {
    let (lo, hi) = volume.occupied_bounds()?;
    let size = [hi[0] - lo[0] + 1, hi[1] - lo[1] + 1, hi[2] - lo[2] + 1];
    let mut out = Volume::new(size);
    for z in 0..size[2] {
        for y in 0..size[1] {
            for x in 0..size[0] {
                out.set(x, y, z, volume.get(x + lo[0], y + lo[1], z + lo[2]));
            }
        }
    }
    Some(out)
}

/// Changes the box size, keeping voxels at their coordinates; whatever falls
/// outside the new box is cut off. Sizes are clamped to the format's 1..=256.
pub fn resize(volume: &Volume, size: [u32; 3]) -> Volume {
    let size = size.map(|s| s.clamp(1, 256));
    let mut out = Volume::new(size);
    let old = volume.size();
    for z in 0..old[2].min(size[2]) {
        for y in 0..old[1].min(size[1]) {
            for x in 0..old[0].min(size[0]) {
                out.set(x, y, z, volume.get(x, y, z));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3x2x1 with distinct colours, so every placement is checkable.
    fn sample() -> Volume {
        let mut v = Volume::new([3, 2, 1]);
        v.set(0, 0, 0, 1);
        v.set(2, 0, 0, 2);
        v.set(0, 1, 0, 3);
        v
    }

    #[test]
    fn a_z_quarter_turn_is_counter_clockwise_from_above() {
        let r = rotate(&sample(), 2);
        assert_eq!(r.size(), [2, 3, 1]);
        // (x, y) -> (sy-1-y, x): +x becomes +y, +y becomes -x.
        assert_eq!(r.get(1, 0, 0), 1);
        assert_eq!(r.get(1, 2, 0), 2);
        assert_eq!(r.get(0, 0, 0), 3);
    }

    #[test]
    fn four_turns_about_any_axis_are_the_identity() {
        let v = sample();
        for axis in 0..3 {
            let mut r = v.clone();
            for _ in 0..4 {
                r = rotate(&r, axis);
            }
            assert_eq!(r, v, "axis {axis}");
        }
    }

    #[test]
    fn mirroring_twice_is_the_identity() {
        let v = sample();
        let m = mirror(&v, 0);
        assert_eq!(m.get(2, 0, 0), 1);
        assert_eq!(mirror(&m, 0), v);
    }

    #[test]
    fn crop_shrinks_to_the_filled_voxels() {
        let mut v = Volume::new([10, 10, 10]);
        v.set(3, 4, 5, 7);
        v.set(5, 4, 6, 8);
        let c = crop(&v).unwrap();
        assert_eq!(c.size(), [3, 1, 2]);
        assert_eq!(c.get(0, 0, 0), 7);
        assert_eq!(c.get(2, 0, 1), 8);
        assert!(crop(&Volume::new([4, 4, 4])).is_none());
    }

    #[test]
    fn resize_keeps_coordinates_and_cuts_the_rest() {
        let r = resize(&sample(), [2, 5, 300]);
        assert_eq!(r.size(), [2, 5, 256]);
        assert_eq!(r.get(0, 0, 0), 1);
        assert_eq!(r.filled_count(), 2, "x = 2 was cut off");
    }
}
