//! Edits as values, so undo is a property of the type.
//!
//! the reference editor does this with ~130 C++ classes deriving from `the reference writer`,
//! each implementing `DoIt`/`Undo`/`Redo`. An enum gives the same structure
//! with one advantage: `match` is exhaustive, so adding a command the compiler
//! cannot undo is a build error rather than a bug found later.

use crate::Volume;
use glam::IVec3;

/// A single reversible change.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Writes palette indices at specific cells, remembering what was there.
    SetVoxels {
        /// `(position, previous, next)` per cell.
        changes: Vec<(IVec3, u8, u8)>,
    },
}

impl Command {
    /// Builds a command from intended writes, capturing the previous values.
    ///
    /// Cells whose value would not change are dropped, so a stroke that
    /// repaints the same colour does not land on the undo stack as a no-op.
    pub fn set_voxels(volume: &Volume, writes: impl IntoIterator<Item = (IVec3, u8)>) -> Self {
        let changes = writes
            .into_iter()
            .filter_map(|(p, next)| {
                let previous = volume.get_i(p.x, p.y, p.z);
                (previous != next).then_some((p, previous, next))
            })
            .collect();
        Self::SetVoxels { changes }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::SetVoxels { changes } => changes.is_empty(),
        }
    }

    pub fn apply(&self, volume: &mut Volume) {
        match self {
            Self::SetVoxels { changes } => {
                for &(p, _, next) in changes {
                    set_signed(volume, p, next);
                }
            }
        }
    }

    pub fn revert(&self, volume: &mut Volume) {
        match self {
            Self::SetVoxels { changes } => {
                // Reverse order, so overlapping writes unwind correctly.
                for &(p, previous, _) in changes.iter().rev() {
                    set_signed(volume, p, previous);
                }
            }
        }
    }

    /// Merges `next` into this command if they belong to one gesture.
    ///
    /// A brush drag fires one command per frame; without this the undo stack
    /// fills with single voxels and one Ctrl+Z barely undoes anything.
    /// the reference editor calls the same idea `CombinePrevCmd`.
    pub fn absorb(&mut self, next: &Command) -> bool {
        match (self, next) {
            (Self::SetVoxels { changes }, Self::SetVoxels { changes: more }) => {
                changes.extend(more.iter().copied());
                true
            }
        }
    }
}

fn set_signed(volume: &mut Volume, p: IVec3, value: u8) {
    if p.x >= 0 && p.y >= 0 && p.z >= 0 {
        volume.set(p.x as u32, p.y as u32, p.z as u32, value);
    }
}

/// Undo and redo stacks.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
    /// Commands pushed while this is true may merge into the previous one.
    coalescing: bool,
}

impl History {
    /// Writes voxels, capturing their previous values, and records the edit.
    ///
    /// This exists because the obvious call does not compile:
    ///
    /// ```compile_fail
    /// # use voxel_core::{Command, History, Volume};
    /// # use glam::IVec3;
    /// # let (mut history, mut volume) = (History::default(), Volume::new([8, 8, 8]));
    /// history.exec(&mut volume, Command::set_voxels(&volume, [(IVec3::ZERO, 1)]));
    /// ```
    ///
    /// The argument borrows the volume immutably while the receiver already
    /// holds it mutably. Taking the writes and building the command inside
    /// keeps both borrows apart, so callers never meet the conflict.
    pub fn set_voxels(
        &mut self,
        volume: &mut Volume,
        writes: impl IntoIterator<Item = (IVec3, u8)>,
    ) {
        let command = Command::set_voxels(volume, writes);
        self.exec(volume, command);
    }

    /// Applies a command and records it.
    pub fn exec(&mut self, volume: &mut Volume, command: Command) {
        if command.is_empty() {
            return;
        }
        command.apply(volume);

        // A new edit invalidates anything that was undone.
        self.redo.clear();

        if self.coalescing {
            if let Some(previous) = self.undo.last_mut() {
                if previous.absorb(&command) {
                    return;
                }
            }
        }
        self.undo.push(command);
        self.coalescing = true;
    }

    /// Ends the current gesture, so the next edit starts a fresh undo entry.
    pub fn end_gesture(&mut self) {
        self.coalescing = false;
    }

    pub fn undo(&mut self, volume: &mut Volume) -> bool {
        self.end_gesture();
        match self.undo.pop() {
            Some(c) => {
                c.revert(volume);
                self.redo.push(c);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self, volume: &mut Volume) -> bool {
        self.end_gesture();
        match self.redo.pop() {
            Some(c) => {
                c.apply(volume);
                self.undo.push(c);
                true
            }
            None => false,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume() -> Volume {
        Volume::new([8, 8, 8])
    }

    #[test]
    fn undo_restores_the_previous_value() {
        let mut v = volume();
        v.set(1, 1, 1, 5);
        let mut h = History::default();

        h.set_voxels(&mut v, [(IVec3::new(1, 1, 1), 9)]);
        assert_eq!(v.get(1, 1, 1), 9);

        assert!(h.undo(&mut v));
        assert_eq!(v.get(1, 1, 1), 5, "should be back to the old colour");

        assert!(h.redo(&mut v));
        assert_eq!(v.get(1, 1, 1), 9);
    }

    #[test]
    fn no_op_writes_never_reach_the_stack() {
        let mut v = volume();
        v.set(2, 2, 2, 7);
        let mut h = History::default();
        h.set_voxels(&mut v, [(IVec3::new(2, 2, 2), 7)]);
        assert!(!h.can_undo(), "repainting the same colour is not an edit");
    }

    #[test]
    fn a_gesture_collapses_into_one_undo_entry() {
        let mut v = volume();
        let mut h = History::default();
        for i in 0..5u32 {
            h.set_voxels(&mut v, [(IVec3::new(i as i32, 0, 0), 3)]);
        }
        assert_eq!(h.undo_depth(), 1, "one drag is one undo");

        h.undo(&mut v);
        for i in 0..5 {
            assert_eq!(v.get(i, 0, 0), 0, "the whole stroke should be gone");
        }
    }

    #[test]
    fn ending_a_gesture_starts_a_new_entry() {
        let mut v = volume();
        let mut h = History::default();
        h.set_voxels(&mut v, [(IVec3::new(0, 0, 0), 1)]);
        h.end_gesture();
        h.set_voxels(&mut v, [(IVec3::new(1, 0, 0), 1)]);
        assert_eq!(h.undo_depth(), 2);
    }

    #[test]
    fn a_new_edit_clears_the_redo_stack() {
        let mut v = volume();
        let mut h = History::default();
        h.set_voxels(&mut v, [(IVec3::new(0, 0, 0), 1)]);
        h.undo(&mut v);
        assert!(h.can_redo());

        h.set_voxels(&mut v, [(IVec3::new(5, 5, 5), 2)]);
        assert!(!h.can_redo(), "redo cannot survive a divergent edit");
    }

    #[test]
    fn undo_on_an_empty_stack_is_harmless() {
        let mut v = volume();
        let mut h = History::default();
        assert!(!h.undo(&mut v));
        assert!(!h.redo(&mut v));
    }
}
