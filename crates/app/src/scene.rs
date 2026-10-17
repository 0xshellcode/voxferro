//! Loading and saving a `.vox` file.

use vox_format::VoxFile;
use voxel_core::{Document, Scene};

/// A loaded file: its scene graph flattened into placed models, plus the
/// palette they share.
pub struct LoadedScene {
    pub name: String,
    pub version: u32,
    /// Models, palette and materials: everything the undo stack can change.
    pub doc: Document,
    /// The file as parsed.
    ///
    /// Kept so saving preserves everything this editor does not model yet --
    /// materials, render settings, cameras, palette notes. Writing from the
    /// scene alone would silently drop all of it.
    source: VoxFile,
    /// Where it came from, for a save with no path. `None` for a new model
    /// that has never been saved.
    pub path: Option<String>,
}

impl LoadedScene {
    pub fn load(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let file = VoxFile::parse(&bytes).map_err(|e| e.to_string())?;

        if file.models.is_empty() {
            return Err("file contains no models".to_owned());
        }

        Ok(Self::from_file(file, Some(path.to_owned())))
    }

    /// A new, empty model of the given size, never saved.
    pub fn new_empty(size: [u32; 3]) -> Self {
        Self::from_file(VoxFile::new_single(size), None)
    }

    pub fn from_file(file: VoxFile, path: Option<String>) -> Self {
        let name = match &path {
            Some(p) => std::path::Path::new(p)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.clone()),
            None => "untitled.vox".to_owned(),
        };
        Self {
            name,
            version: file.version,
            doc: Document {
                scene: Scene::from_file(&file),
                palette: file.palette.clone(),
                materials: file.materials.clone(),
                render: file.render_objects.clone(),
                layers: layers_for(&file),
                meta: file.meta.clone(),
                cameras: file.cameras.clone(),
                palette_order: voxel_core::display_order(file.index_map.as_deref()),
            },
            source: file,
            path,
        }
    }

    /// The animation's frame range: the file's `_anim_range` when it has
    /// one, else from 0 to the last frame anything is keyed on.
    pub fn anim_range(&self) -> (u32, u32) {
        self.doc
            .anim_range()
            .unwrap_or_else(|| (0, voxel_core::anim::last_keyed_frame(&self.doc.scene.nodes)))
    }

    /// Records that the document now lives at `path`, after a save as.
    pub fn set_path(&mut self, path: &str) {
        self.path = Some(path.to_owned());
        if let Some(name) = std::path::Path::new(path).file_name() {
            self.name = name.to_string_lossy().into_owned();
        }
    }

    /// Writes the current state to `path`.
    ///
    /// The edited volumes replace the models of the original file, so every
    /// chunk the editor does not touch travels through untouched.
    pub fn save(&self, path: &str) -> Result<usize, String> {
        let mut file = self.source.clone();

        for (i, volume) in self.doc.scene.models.iter().enumerate() {
            match file.models.get_mut(i) {
                Some(slot) => *slot = volume.to_model(),
                None => file.models.push(volume.to_model()),
            }
        }
        // The graph as edited: moved, added and removed instances.
        file.nodes = self.doc.scene.nodes.clone();
        file.palette = self.doc.palette.clone();
        // The display order, only when it is not the plain one.
        file.index_map = (self.doc.palette_order != voxel_core::identity_order()).then(|| self.doc.palette_order.clone());
        file.materials = self.doc.materials.clone();
        file.render_objects = self.doc.render.clone();
        file.layers = self.doc.layers.clone();
        file.meta = self.doc.meta.clone();
        file.cameras = self.doc.cameras.clone();

        let bytes = file.to_bytes();

        // Write to a temporary file and rename over the target, so an
        // interrupted save cannot leave a half-written model behind.
        let temporary = format!("{path}.tmp");
        std::fs::write(&temporary, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, path).map_err(|e| {
            let _ = std::fs::remove_file(&temporary);
            e.to_string()
        })?;

        Ok(bytes.len())
    }
}

/// The file's layers, plus any the scene graph uses without describing:
/// the editor writes the full table, and a new file has none at all.
fn layers_for(file: &VoxFile) -> Vec<vox_format::Layer> {
    let mut layers = file.layers.clone();
    let used = file.nodes.iter().filter_map(|n| match n {
        vox_format::SceneNode::Transform(t) if t.layer >= 0 => Some(t.layer),
        _ => None,
    });
    // At least the editor's own eight, so there are layers to move to.
    for id in (0..8).chain(used) {
        if !layers.iter().any(|l| l.id == id) {
            layers.push(vox_format::Layer { id, attributes: vox_format::Dict::default() });
        }
    }
    layers.sort_by_key(|l| l.id);
    layers
}

