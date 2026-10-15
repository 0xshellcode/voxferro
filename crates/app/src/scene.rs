//! Loading a `.vox` file into something renderable.

use vox_format::{Palette, VoxFile};
use voxel_core::Volume;

/// A loaded file: its models as dense volumes, plus the palette they share.
pub struct LoadedScene {
    pub name: String,
    pub version: u32,
    pub models: Vec<Volume>,
    pub palette: Palette,
}

impl LoadedScene {
    pub fn load(path: &str) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let file = VoxFile::parse(&bytes).map_err(|e| e.to_string())?;

        if file.models.is_empty() {
            return Err("file contains no models".to_owned());
        }

        let models = file.models.iter().map(Volume::from_model).collect();
        let name = std::path::Path::new(path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_owned());

        Ok(Self {
            name,
            version: file.version,
            models,
            palette: file.palette,
        })
    }

    pub fn total_voxels(&self) -> usize {
        self.models.iter().map(|m| m.filled_count()).sum()
    }
}
