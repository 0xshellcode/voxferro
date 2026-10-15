//! Loading a `.vox` file into something renderable.

use vox_format::{Palette, VoxFile};
use voxel_core::Scene;

/// A loaded file: its scene graph flattened into placed models, plus the
/// palette they share.
pub struct LoadedScene {
    pub name: String,
    pub version: u32,
    pub scene: Scene,
    pub palette: Palette,
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
            scene: Scene::from_file(&file),
            palette: file.palette,
        })
    }
}
