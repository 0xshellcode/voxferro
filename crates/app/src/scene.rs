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
    /// `rOBJ` entries: sun, sky, atmosphere, lens, film, bloom.
    pub render_objects: Vec<vox_format::Dict>,
    /// The file as parsed.
    ///
    /// Kept so saving preserves everything this editor does not model yet --
    /// materials, render settings, cameras, palette notes. Writing from the
    /// scene alone would silently drop all of it.
    source: VoxFile,
    /// Where it came from, for a save with no path.
    pub path: String,
}

impl LoadedScene {
    pub fn load(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let file = VoxFile::parse(&bytes).map_err(|e| e.to_string())?;

        if file.models.is_empty() {
            return Err("file contains no models".to_owned());
        }

        let name = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned());

        Ok(Self {
            name,
            version: file.version,
            doc: Document {
                scene: Scene::from_file(&file),
                palette: file.palette.clone(),
                materials: file.materials.clone(),
            },
            render_objects: file.render_objects.clone(),
            source: file,
            path: path.to_owned(),
        })
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
        file.palette = self.doc.palette.clone();
        file.materials = self.doc.materials.clone();

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
