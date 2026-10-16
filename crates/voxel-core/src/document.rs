//! Everything an edit can change, in one place.

use crate::{Scene, Volume};
use vox_format::{Dict, Layer, Material, Palette};

/// The editable state of an open file: placed models, colours, materials
/// and render settings.
///
/// Commands act on this rather than on a single volume, which is what lets
/// one undo stack hold voxel strokes on any model, palette changes and
/// material changes side by side, in the order they happened.
#[derive(Debug, Clone, Default)]
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
