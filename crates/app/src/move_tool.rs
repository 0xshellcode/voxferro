//! The Move tool: placing instances and groups with the mouse.
//!
//! Dragging a model slides it over a horizontal plane through the point
//! grabbed, in whole voxels; with Shift it slides vertically, over a plane
//! facing the camera; with Cmd, horizontal motion turns it about z, a
//! quarter turn every `TURN_PIXELS`. When the model belongs to the selected
//! group, the group moves instead. X, Y and Z give the target a quarter turn
//! about that axis. A whole drag is one undo step: each frame's move merges
//! into the open gesture.
//!
//! With several objects selected (`objects.rs`), a drag on any of them
//! moves them all, and turns turn them all about their common centre;
//! Shift+click without dragging adds an object to the selection or takes
//! it out.

use crate::{App, edit::Ray};
use eframe::egui;
use glam::{IVec3, Vec3};

/// Horizontal pixels of Cmd+drag per quarter turn.
const TURN_PIXELS: f32 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Across,
    Up,
    Turn,
}

/// A move in progress.
pub struct MoveDrag {
    /// The instance pressed on, to add to or take out of the selection
    /// if a Shift+click ends without moving.
    toggle: Option<usize>,
    motion: Motion,
    /// Where the drag plane was grabbed.
    grab: Vec3,
    /// The vertical plane's normal, for Shift+drag.
    facing: Vec3,
    /// Offset already applied to the node, so each frame applies only the
    /// difference.
    applied: IVec3,
    /// Quarter turns already applied, for Cmd+drag.
    turns: i32,
    start_x: f32,
    centre: IVec3,
}

/// Where a ray meets the plane through `point` with normal `normal`.
fn on_plane(ray: Ray, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let d = ray.direction.dot(normal);
    if d.abs() < 1e-6 {
        return None;
    }
    let t = (point - ray.origin).dot(normal) / d;
    (t > 0.0).then(|| ray.origin + ray.direction * t)
}

impl App {
    /// The transform node a move acts on for instance `index`: the selected
    /// group when it holds the instance, otherwise the instance's own node.
    pub fn move_target(&self, index: usize) -> Option<i32> {
        let scene = &self.scene.as_ref()?.doc.scene;
        if let Some(group) = self.selected_group
            && scene.instances_under(group).contains(&index)
        {
            return Some(group);
        }
        scene.instances.get(index)?.node
    }

    /// Starts a move if the ray hits a model. Returns whether it did.
    pub fn begin_move(&mut self, ray: Ray, pointer_x: f32, modifiers: egui::Modifiers) -> bool {
        let visible = self.visibility();
        let Some(loaded) = &self.scene else { return false };
        let scene = &loaded.doc.scene;
        let Some(pick) = scene.pick(ray.origin, ray.direction, crate::edit::PICK_RANGE, |i| visible.shows(i)) else {
            return false;
        };
        let Some(node) = self.move_target(pick.instance) else {
            self.selected = pick.instance;
            self.console_log.push("this model has no transform node of its own".to_owned());
            return true;
        };
        // A press on an object outside the selection makes it the only
        // one; on one inside, the drag takes them all along.
        if !self.object_nodes().contains(&node) && !modifiers.shift {
            self.objects.clear();
        }
        if !modifiers.shift || self.objects.is_empty() {
            self.selected = pick.instance;
        }
        if Some(node) != self.selected_group {
            self.selected_group = None;
        }
        let centre = self.object_worlds().map_or(IVec3::ZERO, |(_, c)| c);

        let mut facing = (ray.direction * Vec3::new(1.0, 1.0, 0.0)).normalize_or(Vec3::Y);
        if facing.length_squared() < 0.5 {
            facing = Vec3::Y;
        }
        let motion = if modifiers.command {
            Motion::Turn
        } else if modifiers.shift {
            Motion::Up
        } else {
            Motion::Across
        };
        self.history.end_gesture();
        self.move_drag = Some(MoveDrag {
            toggle: modifiers.shift.then_some(pick.instance),
            motion,
            grab: ray.origin + ray.direction * pick.distance,
            facing,
            applied: IVec3::ZERO,
            turns: 0,
            start_x: pointer_x,
            centre,
        });
        true
    }

    /// Follows the cursor while the button is held.
    pub fn update_move(&mut self, ray: Ray, pointer_x: f32) {
        let Some(drag) = &self.move_drag else { return };
        let motion = drag.motion;

        let nodes = match motion {
            Motion::Turn => {
                let wanted = ((pointer_x - drag.start_x) / TURN_PIXELS).trunc() as i32;
                if wanted == drag.turns {
                    return;
                }
                // One quarter per frame; the next frames catch up the rest.
                // Clockwise is three counter-clockwise quarters.
                let (times, step) = if wanted > drag.turns { (1, 1) } else { (3, -1) };
                let result = self.objects_turned(2, times, drag.centre);
                if let Some(drag) = &mut self.move_drag {
                    drag.turns += step;
                    drag.toggle = None;
                }
                result
            }
            Motion::Across | Motion::Up => {
                let normal = if motion == Motion::Across { Vec3::Z } else { drag.facing };
                let Some(point) = on_plane(ray, drag.grab, normal) else { return };
                let mut total = (point - drag.grab).round().as_ivec3();
                if motion == Motion::Across {
                    total.z = 0;
                } else {
                    total.x = 0;
                    total.y = 0;
                }
                let delta = total - drag.applied;
                if delta == IVec3::ZERO {
                    return;
                }
                if let Some(drag) = &mut self.move_drag {
                    drag.applied = total;
                    drag.toggle = None;
                }
                self.objects_moved(delta)
            }
        };
        // Merged into the gesture begun on press: one drag, one undo step.
        if let Err(e) = self.exec_graph(nodes, true) {
            self.console_log.push(format!("error: {e}"));
        }
    }

    pub fn finish_move(&mut self) {
        if let Some(drag) = self.move_drag.take() {
            self.history.end_gesture();
            // A Shift+click that did not move: in or out of the selection.
            if let Some(index) = drag.toggle {
                self.toggle_object(index);
                let message = self.object_select_command(&["list"]);
                self.console_log.push(message);
            }
        }
    }

    /// X, Y, Z: a quarter turn of the selected group or model, in the Move
    /// tool. Skipped while a text field has focus.
    pub fn move_input(&mut self, ctx: &egui::Context) {
        if !self.moving || ctx.egui_wants_keyboard_input() {
            return;
        }
        let axis = ctx.input(|i| {
            if i.modifiers.command {
                return None;
            }
            [(egui::Key::X, "x"), (egui::Key::Y, "y"), (egui::Key::Z, "z")]
                .into_iter()
                .find(|(k, _)| i.key_pressed(*k))
                .map(|(_, a)| a)
        });
        if let Some(axis) = axis {
            let message = if !self.objects.is_empty() {
                self.object_command(&format!("object turn {axis}"))
            } else if self.selected_group.is_some() {
                self.group_command(&format!("node turn {axis}"))
            } else {
                self.instance_command(&format!("instance turn {axis}"))
            };
            self.console_log.push(message);
        }
    }

    /// World bounds of what the Move tool would move: the selected group,
    /// or the selected model.
    pub fn move_target_bounds(&self) -> Option<(Vec3, Vec3)> {
        if !self.moving {
            return None;
        }
        self.bounds_group().or_else(|| self.bounds_of(&[self.selected]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ray_meets_the_ground_plane_where_it_should() {
        let ray = Ray { origin: Vec3::new(0.0, 0.0, 10.0), direction: Vec3::new(0.0, 0.6, -0.8) };
        let hit = on_plane(ray, Vec3::new(5.0, 5.0, 2.0), Vec3::Z).unwrap();
        assert!((hit.z - 2.0).abs() < 1e-5);
        assert!((hit.y - 6.0).abs() < 1e-5, "8 down at slope 0.6/0.8 is 6 across");
    }

    #[test]
    fn a_plane_behind_the_ray_is_missed() {
        let ray = Ray { origin: Vec3::ZERO, direction: Vec3::Z };
        assert!(on_plane(ray, Vec3::new(0.0, 0.0, -5.0), Vec3::Z).is_none());
    }
}
