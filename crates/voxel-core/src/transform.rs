//! Rigid transforms as the `.vox` format stores them.

use glam::{IVec3, Mat3, Vec3};

/// A rotation plus an integer translation.
///
/// Rotations are restricted to the 24 axis-aligned orientations, so every
/// transform maps voxel centres onto voxel centres exactly — no resampling,
/// no drift when transforms compose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub rotation: Mat3,
    pub translation: IVec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            rotation: Mat3::IDENTITY,
            translation: IVec3::ZERO,
        }
    }
}

impl Transform {
    /// Decodes the packed rotation byte stored under a frame's `_r` key.
    ///
    /// The byte is a permutation matrix in eight bits:
    ///
    /// * bits 0-1: which column holds the non-zero of row 0
    /// * bits 2-3: which column holds the non-zero of row 1
    /// * bits 4-6: the sign of each row, set meaning negative
    ///
    /// Row 2's column is whichever one is left over, which is why only two
    /// indices are stored.
    pub fn rotation_from_bits(bits: u8) -> Mat3 {
        let row0 = (bits & 0b11) as usize;
        let row1 = ((bits >> 2) & 0b11) as usize;

        // A malformed byte could name the same column twice; fall back to the
        // identity rather than building a degenerate matrix.
        if row0 > 2 || row1 > 2 || row0 == row1 {
            return Mat3::IDENTITY;
        }
        let row2 = 3 - row0 - row1;

        let sign = |i: u32| if bits & (1 << (4 + i)) != 0 { -1.0 } else { 1.0 };

        let mut m = Mat3::ZERO;
        // glam's Mat3 is column-major: col_mut(c)[r] is row r, column c.
        m.col_mut(row0)[0] = sign(0);
        m.col_mut(row1)[1] = sign(1);
        m.col_mut(row2)[2] = sign(2);
        m
    }

    pub fn from_parts(rotation_bits: u8, translation: [i32; 3]) -> Self {
        Self {
            rotation: Self::rotation_from_bits(rotation_bits),
            translation: IVec3::from_array(translation),
        }
    }

    /// Applies this transform to a point.
    pub fn apply(&self, p: Vec3) -> Vec3 {
        self.rotation * p + self.translation.as_vec3()
    }

    /// Composes with a parent transform: `parent` applied after `self`.
    pub fn then(&self, parent: &Transform) -> Transform {
        Transform {
            rotation: parent.rotation * self.rotation,
            translation: parent.translation
                + (parent.rotation * self.translation.as_vec3()).round().as_ivec3(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bits_decode_to_identity() {
        // 0b0000_0100: row 0 -> column 0, row 1 -> column 1, all positive.
        assert_eq!(Transform::rotation_from_bits(0b0000_0100), Mat3::IDENTITY);
    }

    #[test]
    fn every_valid_byte_is_a_proper_rotation() {
        let mut seen = 0;
        for bits in 0u8..=255 {
            let m = Transform::rotation_from_bits(bits);
            let det = m.determinant();
            assert!(
                (det.abs() - 1.0).abs() < 1e-5,
                "bits {bits:#010b} gave determinant {det}"
            );
            if (det - 1.0).abs() < 1e-5 {
                seen += 1;
            }
        }
        assert!(seen > 0, "some bytes should decode to proper rotations");
    }

    #[test]
    fn malformed_bits_fall_back_to_identity() {
        // Both rows claiming column 0 is not a permutation.
        assert_eq!(Transform::rotation_from_bits(0b0000_0000), Mat3::IDENTITY);
    }

    #[test]
    fn composition_matches_applying_in_order() {
        let child = Transform::from_parts(0b0000_0100, [1, 2, 3]);
        let parent = Transform::from_parts(0b0000_0110, [10, 0, 0]);
        let combined = child.then(&parent);

        let p = Vec3::new(5.0, 7.0, 9.0);
        let stepwise = parent.apply(child.apply(p));
        assert!((combined.apply(p) - stepwise).length() < 1e-4);
    }
}
