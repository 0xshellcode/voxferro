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

    /// Normalises both on-disk conventions into one set of parameters.
    ///
    /// Version 150 writes every key and overloads `_weight`: it is the metal
    /// amount on `_metal`, the transparency on `_glass`, the emission amount
    /// on `_emit`. Version 200 writes only what differs from the defaults,
    /// drops `_type` entirely for diffuse materials, and gives each property
    /// its own key (`_metal`, `_trans`, `_emit`). Callers should not have to
    /// know which file they are looking at.
    pub fn resolve(&self) -> ResolvedMaterial {
        let weight = self.float("_weight");
        let kind = match self.properties.get("_type") {
            Some("_metal") => MaterialKind::Metal,
            Some("_glass") => MaterialKind::Glass,
            Some("_emit") => MaterialKind::Emissive,
            // `_blend` mixes metal, glass and emission by their own keys;
            // classify by whichever is actually present.
            Some("_blend") => {
                if self.float("_emit").is_some_and(|v| v > 0.0) {
                    MaterialKind::Emissive
                } else if self.float("_trans").is_some_and(|v| v > 0.0) {
                    MaterialKind::Glass
                } else if self.float("_metal").is_some_and(|v| v > 0.0) {
                    MaterialKind::Metal
                } else {
                    MaterialKind::Diffuse
                }
            }
            // Absent in version 200 means diffuse; `_media` (volumetric) is
            // rendered as diffuse until participating media are supported.
            _ => MaterialKind::Diffuse,
        };

        // `_ri` is the refractive index itself; `_ior` stores it minus one.
        // A version 200 file carries both (1.3 and 0.3), a version 150 file
        // only `_ior`.
        let ior = self
            .float("_ri")
            .or_else(|| self.float("_ior").map(|v| v + 1.0))
            .unwrap_or(1.3)
            .clamp(1.0, 3.0);

        let amount = |own_key: &str| -> f32 {
            self.float(own_key)
                .or(weight)
                .unwrap_or(1.0)
                .clamp(0.0, 1.0)
        };

        ResolvedMaterial {
            kind,
            roughness: self.float("_rough").unwrap_or(0.1).clamp(0.0, 1.0),
            ior,
            metalness: if kind == MaterialKind::Metal { amount("_metal") } else { 0.0 },
            // Files from a recent version use `_trans`; the editor's own key table
            // (next to `_metal`, `_rough`, `_emit`...) names it `_alpha`.
            transparency: if kind == MaterialKind::Glass {
                self.float("_trans")
                    .or_else(|| self.float("_alpha"))
                    .or(weight)
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            },
            emission: if kind == MaterialKind::Emissive { amount("_emit") } else { 0.0 },
            flux: self.float("_flux").unwrap_or(0.0).max(0.0),
            // Absorption density: `_d` in version 200, `_att` in version 150.
            // Zero means perfectly clear glass whatever its colour.
            density: self
                .float("_d")
                .or_else(|| self.float("_att"))
                .unwrap_or(0.0)
                .max(0.0),
        }
    }
}

/// The broad class of a material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MaterialKind {
    Diffuse = 0,
    Metal = 1,
    Glass = 2,
    Emissive = 3,
}

/// Material parameters in one convention, whatever version wrote them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedMaterial {
    pub kind: MaterialKind,
    /// 0 is a mirror, 1 fully rough.
    pub roughness: f32,
    /// Refractive index, 1.0 or above.
    pub ior: f32,
    /// How metallic, 0..1; zero unless `kind` is `Metal`.
    pub metalness: f32,
    /// How transparent, 0..1; zero unless `kind` is `Glass`.
    pub transparency: f32,
    /// Emission amount, 0..1; zero unless `kind` is `Emissive`.
    pub emission: f32,
    /// Emission power exponent, as the editor's `_flux` slider.
    pub flux: f32,
    /// How strongly a transparent material absorbs per voxel travelled.
    pub density: f32,
}

impl Default for ResolvedMaterial {
    fn default() -> Self {
        Self {
            kind: MaterialKind::Diffuse,
            roughness: 0.1,
            ior: 1.3,
            metalness: 0.0,
            transparency: 0.0,
            emission: 0.0,
            flux: 0.0,
            density: 0.0,
        }
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

#[cfg(test)]
mod material_tests {
    use super::*;

    fn material(pairs: &[(&str, &str)]) -> Material {
        Material {
            id: 1,
            properties: Dict(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
        }
    }

    #[test]
    fn a_version_200_material_without_type_is_diffuse() {
        // Exactly what cars.vox stores: only the non-default keys.
        let m = material(&[("_rough", "0.1"), ("_ior", "0.3"), ("_ri", "1.3"), ("_d", "0.05")]);
        let r = m.resolve();
        assert_eq!(r.kind, MaterialKind::Diffuse);
        assert!((r.ior - 1.3).abs() < 1e-6);
    }

    #[test]
    fn version_150_ior_is_stored_minus_one() {
        let m = material(&[("_type", "_diffuse"), ("_ior", "0.5")]);
        assert!((m.resolve().ior - 1.5).abs() < 1e-6);
    }

    #[test]
    fn version_150_weight_means_whatever_the_type_needs() {
        let metal = material(&[("_type", "_metal"), ("_weight", "0.7"), ("_rough", "0.2")]).resolve();
        assert_eq!(metal.kind, MaterialKind::Metal);
        assert!((metal.metalness - 0.7).abs() < 1e-6);
        assert_eq!(metal.transparency, 0.0);

        let glass = material(&[("_type", "_glass"), ("_weight", "0.4")]).resolve();
        assert_eq!(glass.kind, MaterialKind::Glass);
        assert!((glass.transparency - 0.4).abs() < 1e-6);
        assert_eq!(glass.metalness, 0.0);
    }

    #[test]
    fn glass_transparency_accepts_alpha_and_trans() {
        let a = material(&[("_type", "_glass"), ("_alpha", "0.6")]).resolve();
        assert!((a.transparency - 0.6).abs() < 1e-6);
        let t = material(&[("_type", "_glass"), ("_trans", "0.3"), ("_alpha", "0.6")]).resolve();
        assert!((t.transparency - 0.3).abs() < 1e-6, "_trans takes precedence");
    }

    #[test]
    fn version_200_own_keys_win_over_weight() {
        let m = material(&[("_type", "_metal"), ("_metal", "0.9"), ("_weight", "0.1")]).resolve();
        assert!((m.metalness - 0.9).abs() < 1e-6);
    }

    #[test]
    fn room_vox_emitter_resolves() {
        // Index 164 of room.vox, verbatim.
        let m = material(&[
            ("_type", "_emit"), ("_weight", "1"), ("_rough", "0.1"), ("_spec", "0.5"),
            ("_spec_p", "0.5"), ("_ior", "0.3"), ("_att", "0"), ("_flux", "2"), ("_ldr", "0"),
        ])
        .resolve();
        assert_eq!(m.kind, MaterialKind::Emissive);
        assert_eq!(m.emission, 1.0);
        assert_eq!(m.flux, 2.0);
    }

    #[test]
    fn density_reads_either_convention() {
        assert_eq!(material(&[("_d", "0.05")]).resolve().density, 0.05);
        assert_eq!(material(&[("_type", "_glass"), ("_att", "0.3")]).resolve().density, 0.3);
        assert_eq!(material(&[("_type", "_glass")]).resolve().density, 0.0, "clear by default");
    }

    #[test]
    fn nonsense_values_are_clamped() {
        let m = material(&[("_type", "_glass"), ("_rough", "7"), ("_ri", "0.2"), ("_weight", "-3")]).resolve();
        assert_eq!(m.roughness, 1.0);
        assert_eq!(m.ior, 1.0);
        assert_eq!(m.transparency, 0.0);
    }
}
