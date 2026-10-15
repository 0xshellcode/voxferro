//! The data a `.vox` file carries.

use crate::Dict;

/// A voxel inside a model, in the model's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voxel {
    pub x: u8,
    pub y: u8,
    pub z: u8,
    /// Palette index. Never 0 in a file: 0 means empty.
    pub index: u8,
}

/// One `SIZE` + `XYZI` pair: a sparse list of voxels and the box holding them.
#[derive(Debug, Clone, Default)]
pub struct Model {
    pub size: [u32; 3],
    pub voxels: Vec<Voxel>,
}

/// An RGBA colour as stored in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// The 256-entry colour table.
///
/// The `RGBA` chunk is stored shifted by one: entry `i` of the chunk is the
/// colour of voxel index `i + 1`. [`Palette::color`] hides that, so callers
/// index by the value they read out of a voxel.
#[derive(Debug, Clone)]
pub struct Palette {
    /// Indexed by voxel index: `colors[i]` is the colour of voxel index `i`.
    /// Slot 0 is the empty voxel and is never drawn.
    colors: [Color; 256],
}

impl Palette {
    /// Builds a palette from a raw `RGBA` chunk, undoing the shift.
    pub fn from_chunk(entries: &[Color; 256]) -> Self {
        let mut colors = [Color::default(); 256];
        for (i, c) in entries.iter().enumerate() {
            // Chunk entry i describes voxel index i+1; index 255 wraps to 0,
            // the empty slot, which is never read.
            colors[(i + 1) & 0xff] = *c;
        }
        Self { colors }
    }

    /// Colour of a voxel index. Index 0 (empty) yields a fully transparent
    /// colour.
    pub fn color(&self, index: u8) -> Color {
        self.colors[index as usize]
    }

    /// All 256 slots, indexed by voxel index. Ready to upload to the GPU.
    pub fn as_slice(&self) -> &[Color; 256] {
        &self.colors
    }
}

impl Default for Palette {
    /// the reference editor's default ramp is not reproduced here; a file without an
    /// `RGBA` chunk gets a visible grey so missing palettes are obvious rather
    /// than invisible.
    fn default() -> Self {
        let mut colors = [Color::default(); 256];
        for (i, c) in colors.iter_mut().enumerate().skip(1) {
            let v = (i as u8).max(32);
            *c = Color { r: v, g: v, b: v, a: 255 };
        }
        Self { colors }
    }
}

/// `nTRN`: places its single child, optionally per animation frame.
#[derive(Debug, Clone, Default)]
pub struct TransformNode {
    pub id: i32,
    pub attributes: Dict,
    pub child: i32,
    pub layer: i32,
    /// One entry per animation frame; static models have exactly one.
    pub frames: Vec<Dict>,
}

impl TransformNode {
    /// Translation of a frame, from its `_t` key. Absent means the origin.
    pub fn translation(&self, frame: usize) -> [i32; 3] {
        self.frames
            .get(frame)
            .and_then(|f| f.get_parsed::<i32, 3>("_t"))
            .unwrap_or([0, 0, 0])
    }

    /// Packed rotation byte of a frame, from its `_r` key.
    ///
    /// Decoding lives in `voxel-core`, which owns the matrix types; keeping the
    /// raw byte here means this crate stays free of a math dependency.
    pub fn rotation_bits(&self, frame: usize) -> u8 {
        self.frames
            .get(frame)
            .and_then(|f| f.get("_r"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0b0000_0100)
    }
}

/// `nGRP`: groups any number of children.
#[derive(Debug, Clone, Default)]
pub struct GroupNode {
    pub id: i32,
    pub attributes: Dict,
    pub children: Vec<i32>,
}

/// A model reference inside a shape node.
#[derive(Debug, Clone, Default)]
pub struct ShapeModel {
    pub model_id: i32,
    pub attributes: Dict,
}

/// `nSHP`: a leaf that points at one or more models.
#[derive(Debug, Clone, Default)]
pub struct ShapeNode {
    pub id: i32,
    pub attributes: Dict,
    pub models: Vec<ShapeModel>,
}

/// A node of the scene graph, as stored in the file.
#[derive(Debug, Clone)]
pub enum SceneNode {
    Transform(TransformNode),
    Group(GroupNode),
    Shape(ShapeNode),
}

impl SceneNode {
    pub fn id(&self) -> i32 {
        match self {
            Self::Transform(n) => n.id,
            Self::Group(n) => n.id,
            Self::Shape(n) => n.id,
        }
    }
}

/// `LAYR`: one of the eight layers, carrying its name and visibility.
#[derive(Debug, Clone, Default)]
pub struct Layer {
    pub id: i32,
    pub attributes: Dict,
}

impl Layer {
    pub fn name(&self) -> Option<&str> {
        self.attributes.get("_name")
    }

    /// Layers store `_hidden` as "1" when hidden; absent means visible.
    pub fn is_hidden(&self) -> bool {
        self.attributes.get("_hidden").is_some_and(|v| v == "1")
    }
}

/// `MATL`: the material of one palette index.
#[derive(Debug, Clone, Default)]
pub struct Material {
    pub id: i32,
    pub properties: Dict,
}

impl Material {
    /// `_diffuse`, `_metal`, `_glass`, `_emit`, and so on.
    pub fn kind(&self) -> &str {
        self.properties.get("_type").unwrap_or("_diffuse")
    }

    pub fn float(&self, key: &str) -> Option<f32> {
        self.properties.get(key)?.parse().ok()
    }
}

/// `rCAM`: a saved camera. Only written by format version 200 and later.
#[derive(Debug, Clone, Default)]
pub struct Camera {
    pub id: i32,
    pub attributes: Dict,
}

impl Camera {
    /// `pers`, `orth`, `free` — matching the modes the editor exposes.
    pub fn mode(&self) -> &str {
        self.attributes.get("_mode").unwrap_or("pers")
    }
}
