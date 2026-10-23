//! Everything an edit can change, in one place.

use crate::{Scene, Volume};
use vox_format::{Dict, Layer, Material, Palette};

/// The editable state of an open file: placed models, colours, materials
/// and render settings.
///
/// Commands act on this rather than on a single volume, which is what lets
/// one undo stack hold voxel strokes on any model, palette changes and
/// material changes side by side, in the order they happened.
#[derive(Debug, Clone)]
pub struct Document {
    pub scene: Scene,
    pub palette: Palette,
    /// `MATL` entries; a palette index without one is plain diffuse.
    pub materials: Vec<Material>,
    /// `rOBJ` render settings: sun, sky, ground, film... Kept as the file's
    /// key/value objects, so keys this editor does not model survive.
    pub render: Vec<Dict>,
    /// `LAYR` entries: name, colour and visibility of each layer.
    pub layers: Vec<Layer>,
    /// `META` objects, such as `_anim_range`.
    pub meta: Vec<Dict>,
    /// `rCAM` saved cameras (see `saved_camera`).
    pub cameras: Vec<vox_format::Camera>,
    /// The palette as shown: display slot to colour index (`IMAP`).
    /// Sorting reorders this, never the colours, so voxels keep theirs.
    pub palette_order: Box<[u8; 256]>,
    /// The palette's notes (`NOTE`): one line of text for each row of eight
    /// colours, 32 in all in the files the reference editor writes.
    pub notes: Vec<String>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            scene: Scene::default(),
            palette: Palette::default(),
            materials: Vec::new(),
            render: Vec::new(),
            layers: Vec::new(),
            meta: Vec::new(),
            cameras: Vec::new(),
            palette_order: identity_order(),
            notes: Vec::new(),
        }
    }
}

/// Slot k shows colour k.
pub fn identity_order() -> Box<[u8; 256]> {
    Box::new(std::array::from_fn(|k| k as u8))
}

/// A file's `IMAP` as the display order, when it is one: every colour in
/// exactly one slot. A damaged map -- a colour twice, another nowhere --
/// would hide colours from the palette, so it falls back to the identity.
pub fn display_order(map: Option<&[u8; 256]>) -> Box<[u8; 256]> {
    let Some(map) = map else { return identity_order() };
    let mut seen = [false; 256];
    for &i in map {
        if std::mem::replace(&mut seen[i as usize], true) {
            return identity_order();
        }
    }
    Box::new(*map)
}

impl Document {
    pub fn model_mut(&mut self, index: usize) -> Option<&mut Volume> {
        self.scene.models.get_mut(index)
    }

    /// Whether a layer is switched off. A layer the file does not describe
    /// is visible, and so is "no layer" (-1).
    pub fn layer_hidden(&self, id: i32) -> bool {
        self.layers.iter().any(|l| l.id == id && l.is_hidden())
    }

    /// The saved animation range, `_anim_range "first last"`.
    pub fn anim_range(&self) -> Option<(u32, u32)> {
        self.meta
            .iter()
            .find_map(|m| m.get_parsed::<u32, 2>("_anim_range"))
            .filter(|[a, b]| b >= a)
            .map(|[a, b]| (a, b))
    }

    /// The meta objects with `_anim_range` set, in the object that already
    /// had it or a new one.
    pub fn with_anim_range(&self, first: u32, last: u32) -> Vec<Dict> {
        let mut meta = self.meta.clone();
        let value = format!("{first} {last}");
        match meta.iter_mut().find(|m| m.get("_anim_range").is_some()) {
            Some(m) => m.set("_anim_range", value),
            None => meta.push(Dict(vec![("_anim_range".into(), value)])),
        }
        meta
    }

    /// The material of a palette index, if it has one.
    pub fn material(&self, index: u8) -> Option<&Material> {
        self.materials.iter().find(|m| m.id == index as i32)
    }

    /// Replaces, adds or removes the material of a palette index.
    pub fn put_material(&mut self, index: u8, material: Option<Material>) {
        let slot = self.materials.iter().position(|m| m.id == index as i32);
        match (slot, material) {
            (Some(i), Some(mut m)) => {
                m.id = index as i32;
                self.materials[i] = m;
            }
            (Some(i), None) => {
                self.materials.remove(i);
            }
            (None, Some(mut m)) => {
                m.id = index as i32;
                self.materials.push(m);
                self.materials.sort_by_key(|m| m.id);
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;

    #[test]
    fn only_a_permutation_is_a_display_order() {
        let mut reversed = [0u8; 256];
        for (k, slot) in reversed.iter_mut().enumerate() {
            *slot = (255 - k) as u8;
        }
        assert_eq!(*display_order(Some(&reversed)), reversed);
        assert_eq!(display_order(None), identity_order());
        reversed[3] = reversed[4];
        assert_eq!(display_order(Some(&reversed)), identity_order());
    }
}
