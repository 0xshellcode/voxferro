//! Voxel volume, scene graph and command stack.
//!
//! Phases 4 and 5 grow this crate. Two decisions are already fixed, because
//! both are hard to retrofit:
//!
//! * The scene graph stores nodes in a flat arena addressed by `NodeId`, not
//!   by `Rc<RefCell<..>>`. Parent and child links are plain indices, so the
//!   graph serializes directly and never fights the borrow checker.
//! * Every edit is a `Command` value that knows how to apply and revert
//!   itself, so undo is a property of the type rather than a parallel code
//!   path that can drift out of sync.

/// Index of a node in the scene arena.
///
/// A newtype rather than a bare `u32`: it cannot be confused with a voxel
/// index or a layer number at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// A dense voxel volume. Each cell holds a palette index, where 0 means empty.
#[derive(Debug, Clone, Default)]
pub struct Volume {
    size: [u32; 3],
    voxels: Vec<u8>,
}

impl Volume {
    pub fn new(size: [u32; 3]) -> Self {
        let count = (size[0] as usize) * (size[1] as usize) * (size[2] as usize);
        Self {
            size,
            voxels: vec![0; count],
        }
    }

    pub fn size(&self) -> [u32; 3] {
        self.size
    }

    fn index(&self, x: u32, y: u32, z: u32) -> Option<usize> {
        if x >= self.size[0] || y >= self.size[1] || z >= self.size[2] {
            return None;
        }
        Some((x + y * self.size[0] + z * self.size[0] * self.size[1]) as usize)
    }

    /// Palette index at a coordinate; 0 for empty or out of bounds.
    ///
    /// Out-of-bounds reads answer "empty" rather than erroring, because the
    /// mesher queries neighbours across the volume edge on every face.
    pub fn get(&self, x: u32, y: u32, z: u32) -> u8 {
        self.index(x, y, z).map_or(0, |i| self.voxels[i])
    }

    pub fn set(&mut self, x: u32, y: u32, z: u32, value: u8) {
        if let Some(i) = self.index(x, y, z) {
            self.voxels[i] = value;
        }
    }

    pub fn is_empty_at(&self, x: u32, y: u32, z: u32) -> bool {
        self.get(x, y, z) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_round_trips_a_voxel() {
        let mut v = Volume::new([3, 3, 3]);
        assert!(v.is_empty_at(1, 1, 1));
        v.set(1, 1, 1, 42);
        assert_eq!(v.get(1, 1, 1), 42);
    }

    #[test]
    fn out_of_bounds_reads_as_empty() {
        let v = Volume::new([2, 2, 2]);
        assert_eq!(v.get(9, 0, 0), 0);
        assert!(v.is_empty_at(0, 0, 9));
    }
}
