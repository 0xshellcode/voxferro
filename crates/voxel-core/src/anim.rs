//! Animation: what the scene graph holds at a given frame.
//!
//! The format keys animation by frame number, `_f`, on two kinds of node:
//!
//! - a transform node's frames are keyframes, each with `_f` and the `_t`
//!   and `_r` of that key;
//! - a shape node's models are keyed the same way: which model is shown from
//!   that frame on. With `_loop` set on the shape, its models repeat.
//!
//! A frame without `_f` is frame 0, which is how every static file reads.
//!
//! Between keys, translation is interpolated linearly. The a recent version binary has
//! no symbols to find its evaluator by, but a file it saved pins this down:
//! `T-Rex-m.vox` keys x = 44 at frame 11 and x = 148 at frame 19, and caches
//! the current placement as `_frm_t "122 0 13"` -- exactly linear at frame
//! 17 (44 + 6/8 * 104); stepped keys could only give 44 or 148, and an
//! ease curve no whole frame at all. `_loop` exists on transform nodes too
//! (the a recent version writer emits it for both).

use crate::Transform;
use vox_format::{Dict, ShapeModel, ShapeNode, TransformNode};

fn frame_of(d: &Dict) -> u32 {
    d.get("_f").and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn looping(attributes: &Dict) -> bool {
    attributes.get("_loop").is_some_and(|v| v != "0")
}

fn placement(d: &Dict) -> Transform {
    let t = d.get_parsed::<i32, 3>("_t").unwrap_or([0, 0, 0]);
    let r = d.get("_r").and_then(|v| v.parse().ok()).unwrap_or(0b0000_0100);
    Transform::from_parts(r, t)
}

/// The keyframes of a transform node, by frame, earliest first.
pub fn keyframes(node: &TransformNode) -> Vec<(u32, Transform)> {
    let mut keys: Vec<(u32, Transform)> = node.frames.iter().map(|d| (frame_of(d), placement(d))).collect();
    // Stable: of two keys on one frame, the first listed wins below.
    keys.sort_by_key(|k| k.0);
    keys.dedup_by_key(|k| k.0);
    keys
}

/// Whether a transform node carries keys (`_f`), rather than one placement.
pub fn is_keyed(node: &TransformNode) -> bool {
    node.frames.iter().any(|d| d.get("_f").is_some())
}

/// A transform node's placement at `frame`, stepping from key to key
/// without interpolating: the key on that frame, else the one before, else
/// the first; `_loop` wraps the frame first. How the reference editor's unpack places
/// each frame.
pub fn key_at(node: &TransformNode, frame: u32) -> Transform {
    let keys = keyframes(node);
    let Some(&(_, first)) = keys.first() else { return Transform::default() };
    let frame = if looping(&node.attributes) { frame % (keys.last().map_or(0, |k| k.0) + 1) } else { frame };
    keys.iter().rev().find(|k| k.0 <= frame).map_or(first, |k| k.1)
}

/// A transform node's own placement at `frame`.
///
/// Before the first key, the first key holds; after the last, the last.
/// Between two keys, see [`between`].
pub fn local_at(node: &TransformNode, frame: u32) -> Transform {
    let keys = keyframes(node);
    let Some(&(_, first)) = keys.first() else { return Transform::default() };
    let frame = if looping(&node.attributes) {
        frame % (keys.last().map_or(0, |k| k.0) + 1)
    } else {
        frame
    };
    let next = keys.iter().position(|k| k.0 > frame);
    match next {
        None => keys.last().map_or(first, |k| k.1),
        Some(0) => first,
        Some(i) => between(keys[i - 1], keys[i], frame),
    }
}

/// The placement between two keyframes.
///
/// Translation runs linearly from one key to the next, rounded to whole
/// voxels; rotation holds the earlier key's until the next key is reached
/// (an axis-aligned rotation has no halfway).
pub fn between(a: (u32, Transform), b: (u32, Transform), frame: u32) -> Transform {
    let span = (b.0 - a.0).max(1) as f32;
    let t = (frame - a.0) as f32 / span;
    let from = a.1.translation.as_vec3();
    let to = b.1.translation.as_vec3();
    Transform {
        rotation: a.1.rotation,
        translation: (from + (to - from) * t).round().as_ivec3(),
    }
}

/// Writes a placement at `frame`.
///
/// A node with a single frame and no `_f` is static: editing it at frame 0
/// keeps it static. Otherwise the key on `frame` is updated, or a new key is
/// added there -- editing an animated node at another frame keys it, as the
/// editor does.
pub fn set_local_at(node: &mut TransformNode, frame: u32, local: Transform) {
    let keyed = node.frames.iter().any(|d| d.get("_f").is_some());
    if !keyed && frame == 0 {
        node.set_placement(local.translation.to_array(), local.rotation_bits());
        return;
    }
    if !keyed {
        // Turning a static node into an animated one: its placement so far
        // becomes the key at frame 0.
        for d in &mut node.frames {
            d.set("_f", "0");
        }
        if node.frames.is_empty() {
            node.frames.push(Dict(vec![("_f".into(), "0".into())]));
        }
    }
    let index = match node.frames.iter().position(|d| frame_of(d) == frame) {
        Some(i) => i,
        None => {
            let at = node.frames.iter().position(|d| frame_of(d) > frame).unwrap_or(node.frames.len());
            node.frames.insert(at, Dict(vec![("_f".into(), frame.to_string())]));
            at
        }
    };
    let d = &mut node.frames[index];
    let t = local.translation;
    d.set("_t", format!("{} {} {}", t.x, t.y, t.z));
    let bits = local.rotation_bits();
    if bits == 0b0000_0100 {
        d.remove("_r");
    } else {
        d.set("_r", bits.to_string());
    }
}

/// Removes the key on `frame`, if it is not the node's only one.
pub fn remove_key(node: &mut TransformNode, frame: u32) -> bool {
    if node.frames.len() < 2 {
        return false;
    }
    let before = node.frames.len();
    node.frames.retain(|d| frame_of(d) != frame);
    node.frames.len() != before
}

/// Moves the key on frame `from` to frame `to`. Refused (false) when there
/// is no key on `from` or one already on `to`: a key never overwrites
/// another.
pub fn move_key(node: &mut TransformNode, from: u32, to: u32) -> bool {
    if from == to || node.frames.iter().any(|d| frame_of(d) == to) {
        return false;
    }
    // A static node has nothing to move in time.
    if node.frames.len() < 2 {
        return false;
    }
    let Some(d) = node.frames.iter_mut().find(|d| frame_of(d) == from) else { return false };
    d.set("_f", to.to_string());
    node.frames.sort_by_key(frame_of);
    true
}

/// Moves a shape's model keyed on frame `from` to frame `to`, under the same
/// rule as [`move_key`].
pub fn move_shape_frame(shape: &mut ShapeNode, from: u32, to: u32) -> bool {
    if from == to || shape.models.len() < 2 || shape.models.iter().any(|m| frame_of(&m.attributes) == to) {
        return false;
    }
    let Some(m) = shape.models.iter_mut().find(|m| frame_of(&m.attributes) == from) else { return false };
    m.attributes.set("_f", to.to_string());
    shape.models.sort_by_key(|m| frame_of(&m.attributes));
    true
}

/// The model a shape shows at `frame`: the latest one keyed at or before it,
/// with `_loop` wrapping the frame around the shape's own length.
pub fn model_at(shape: &ShapeNode, frame: u32) -> Option<&ShapeModel> {
    let last = shape.models.iter().map(|m| frame_of(&m.attributes)).max()?;
    let frame = if looping(&shape.attributes) { frame % (last + 1) } else { frame };
    shape
        .models
        .iter()
        .filter(|m| frame_of(&m.attributes) <= frame)
        .max_by_key(|m| frame_of(&m.attributes))
        .or_else(|| shape.models.first())
}

/// The last frame anything in the graph is keyed on.
pub fn last_keyed_frame(nodes: &[vox_format::SceneNode]) -> u32 {
    nodes
        .iter()
        .flat_map(|n| match n {
            vox_format::SceneNode::Transform(t) => t.frames.iter().map(frame_of).collect::<Vec<_>>(),
            vox_format::SceneNode::Shape(s) => s.models.iter().map(|m| frame_of(&m.attributes)).collect(),
            vox_format::SceneNode::Group(_) => Vec::new(),
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;

    fn dict(pairs: &[(&str, &str)]) -> Dict {
        Dict(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    /// T-Rex-m.vox's walking transform.
    fn walk() -> TransformNode {
        TransformNode {
            id: 2,
            attributes: Dict::default(),
            child: 3,
            layer: 0,
            frames: vec![
                dict(&[("_f", "0"), ("_t", "0 0 13")]),
                dict(&[("_f", "11"), ("_t", "44 0 13")]),
                dict(&[("_f", "19"), ("_r", "17"), ("_t", "148 0 13")]),
            ],
        }
    }

    #[test]
    fn keys_hold_at_their_frames_and_beyond_the_ends() {
        let t = walk();
        assert_eq!(local_at(&t, 0).translation, IVec3::new(0, 0, 13));
        assert_eq!(local_at(&t, 11).translation, IVec3::new(44, 0, 13));
        assert_eq!(local_at(&t, 19).translation, IVec3::new(148, 0, 13));
        assert_eq!(local_at(&t, 30).translation, IVec3::new(148, 0, 13), "after the last key");
        assert_eq!(local_at(&t, 19).rotation, Transform::rotation_from_bits(17));
    }

    #[test]
    fn interpolation_matches_the_editors_saved_placement() {
        // T-Rex-m.vox caches `_frm_t "122 0 13"`: linear at frame 17.
        assert_eq!(local_at(&walk(), 17).translation, IVec3::new(122, 0, 13));
        // Rotation holds until its key.
        assert_eq!(local_at(&walk(), 18).rotation, Transform::rotation_from_bits(4));
    }

    #[test]
    fn a_looping_transform_wraps_around() {
        let mut t = walk();
        t.attributes.set("_loop", "1");
        assert_eq!(local_at(&t, 20 + 11).translation, local_at(&t, 11).translation);
    }

    #[test]
    fn a_static_file_reads_as_frame_zero() {
        let t = TransformNode { frames: vec![dict(&[("_t", "1 2 3")])], ..Default::default() };
        assert_eq!(local_at(&t, 7).translation, IVec3::new(1, 2, 3));
    }

    #[test]
    fn editing_at_a_frame_adds_a_key_there() {
        let mut t = TransformNode { frames: vec![dict(&[("_t", "0 0 0")])], ..Default::default() };
        let moved = Transform { translation: IVec3::new(10, 0, 0), ..Default::default() };
        set_local_at(&mut t, 0, moved);
        assert_eq!(t.frames.len(), 1, "frame 0 of a static node stays static");
        assert!(t.frames[0].get("_f").is_none());

        set_local_at(&mut t, 8, Transform::default());
        assert_eq!(keyframes(&t).iter().map(|k| k.0).collect::<Vec<_>>(), vec![0, 8]);
        assert_eq!(local_at(&t, 0).translation, IVec3::new(10, 0, 0), "the old placement is key 0");
        assert!(remove_key(&mut t, 8));
        assert!(!remove_key(&mut t, 0), "the last key stays");
    }

    #[test]
    fn looping_shapes_repeat_their_models() {
        let model = |id: i32, f: &str| ShapeModel { model_id: id, attributes: dict(&[("_f", f)]) };
        let mut shape = ShapeNode {
            id: 3,
            attributes: dict(&[("_loop", "1")]),
            models: vec![model(0, "0"), model(1, "1"), model(2, "2"), model(3, "3")],
        };
        assert_eq!(model_at(&shape, 2).map(|m| m.model_id), Some(2));
        assert_eq!(model_at(&shape, 5).map(|m| m.model_id), Some(1), "5 wraps to 1");
        shape.attributes = Dict::default();
        assert_eq!(model_at(&shape, 5).map(|m| m.model_id), Some(3), "without _loop the last holds");
    }

    #[test]
    fn keys_move_but_never_onto_each_other() {
        let mut t = walk();
        assert!(move_key(&mut t, 11, 7));
        assert_eq!(keyframes(&t).iter().map(|k| k.0).collect::<Vec<_>>(), vec![0, 7, 19]);
        assert_eq!(local_at(&t, 7).translation, IVec3::new(44, 0, 13), "the key's placement moved with it");
        assert!(!move_key(&mut t, 7, 19), "19 is taken");
        assert!(!move_key(&mut t, 3, 4), "no key on 3");
        // Moving past a neighbour reorders the keys.
        assert!(move_key(&mut t, 0, 25));
        assert_eq!(keyframes(&t).iter().map(|k| k.0).collect::<Vec<_>>(), vec![7, 19, 25]);
    }

    #[test]
    fn shape_frames_move_too() {
        let model = |id: i32, f: &str| ShapeModel { model_id: id, attributes: dict(&[("_f", f)]) };
        let mut shape = ShapeNode { id: 3, attributes: Dict::default(), models: vec![model(0, "0"), model(1, "4")] };
        assert!(move_shape_frame(&mut shape, 4, 9));
        assert_eq!(model_at(&shape, 8).map(|m| m.model_id), Some(0));
        assert_eq!(model_at(&shape, 9).map(|m| m.model_id), Some(1));
        assert!(!move_shape_frame(&mut shape, 9, 0), "0 is taken");
    }
}

