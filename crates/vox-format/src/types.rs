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

    /// Replaces the colour of a voxel index. Index 0 is the empty slot and is
    /// never drawn, so writing it is accepted and ignored.
    pub fn set_color(&mut self, index: u8, color: Color) {
        if index != 0 {
            self.colors[index as usize] = color;
        }
    }

    /// All 256 slots, indexed by voxel index. Ready to upload to the GPU.
    pub fn as_slice(&self) -> &[Color; 256] {
        &self.colors
    }
}

impl Palette {
    /// The default palette from the format's public specification, which a
    /// file without an `RGBA` chunk is meant to use.
    ///
    /// It is generated rather than tabulated: the first 215 entries are a
    /// 6x6x6 colour cube (each channel from 0xff down to 0x00 in steps of
    /// 0x33, blue fastest, black left out), then four 10-step ramps of red,
    /// green, blue and grey (0xee, 0xdd, 0xbb, 0xaa, 0x88, 0x77, 0x55, 0x44,
    /// 0x22, 0x11).
    pub fn specification_default() -> Self {
        let mut colors = [Color::default(); 256];
        let levels = [0xff, 0xcc, 0x99, 0x66, 0x33, 0x00];
        let mut i = 1;
        for r in levels {
            for g in levels {
                for b in levels {
                    if r == 0 && g == 0 && b == 0 {
                        continue;
                    }
                    colors[i] = Color { r, g, b, a: 255 };
                    i += 1;
                }
            }
        }
        let ramp = [0xee, 0xdd, 0xbb, 0xaa, 0x88, 0x77, 0x55, 0x44, 0x22, 0x11];
        for channel in 0..4 {
            for v in ramp {
                colors[i] = match channel {
                    0 => Color { r: v, g: 0, b: 0, a: 255 },
                    1 => Color { r: 0, g: v, b: 0, a: 255 },
                    2 => Color { r: 0, g: 0, b: v, a: 255 },
                    _ => Color { r: v, g: v, b: v, a: 255 },
                };
                i += 1;
            }
        }
        debug_assert_eq!(i, 256);
        Self { colors }
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
#[derive(Debug, Clone, Default, PartialEq)]
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

    /// Sets the first frame's placement, creating the frame if the node had
    /// none. Other keys of the frame (such as `_f`) are kept. `_r` is left
    /// out for the identity, as the editor writes it.
    pub fn set_placement(&mut self, translation: [i32; 3], rotation_bits: u8) {
        if self.frames.is_empty() {
            self.frames.push(Dict::default());
        }
        let frame = &mut self.frames[0];
        frame.set("_t", format!("{} {} {}", translation[0], translation[1], translation[2]));
        if rotation_bits == 0b0000_0100 {
            frame.remove("_r");
        } else {
            frame.set("_r", rotation_bits.to_string());
        }
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
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GroupNode {
    pub id: i32,
    pub attributes: Dict,
    pub children: Vec<i32>,
}

/// A model reference inside a shape node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapeModel {
    pub model_id: i32,
    pub attributes: Dict,
}

/// `nSHP`: a leaf that points at one or more models.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapeNode {
    pub id: i32,
    pub attributes: Dict,
    pub models: Vec<ShapeModel>,
}

/// The smallest graph that places every model: a root transform, a group,
/// then a transform and shape per model -- the arrangement the editor writes
/// for a new scene. Ids: 0 root, 1 group, then pairs from 2.
///
/// Each model sits on the ground the way the editor places it: every model in
/// the sample corpus has `_t.z == size.z / 2`. Centred on the origin instead,
/// half of it would be under the render view's ground plane.
pub fn minimal_graph(models: &[Model]) -> Vec<SceneNode> {
    let count = models.len().max(1);
    let mut nodes = vec![
        SceneNode::Transform(TransformNode {
            id: 0,
            attributes: Dict::default(),
            child: 1,
            layer: -1,
            frames: vec![Dict::default()],
        }),
        SceneNode::Group(GroupNode {
            id: 1,
            attributes: Dict::default(),
            children: (0..count).map(|i| 2 + i as i32 * 2).collect(),
        }),
    ];
    for i in 0..count {
        let transform_id = 2 + i as i32 * 2;
        let lift = models.get(i).map_or(0, |m| m.size[2] / 2);
        nodes.push(SceneNode::Transform(TransformNode {
            id: transform_id,
            attributes: Dict::default(),
            child: transform_id + 1,
            layer: 0,
            frames: vec![Dict(vec![("_t".to_owned(), format!("0 0 {lift}"))])],
        }));
        nodes.push(SceneNode::Shape(ShapeNode {
            id: transform_id + 1,
            attributes: Dict::default(),
            models: vec![ShapeModel { model_id: i as i32, attributes: Dict::default() }],
        }));
    }
    nodes
}

/// A node of the scene graph, as stored in the file.
#[derive(Debug, Clone, PartialEq)]
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

/// `LAYR`: a layer, carrying its name, colour and visibility. Version 150
/// files have eight, version 200 files thirty-two.
#[derive(Debug, Clone, Default, PartialEq)]
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
#[derive(Debug, Clone, Default, PartialEq)]
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
            // Cloud in the editor's panel: a participating medium.
            Some("_media") => MaterialKind::Media,
            // `_blend` mixes metal, glass and emission by their own keys.
            Some("_blend") => MaterialKind::Blend,
            // Absent in version 200 means diffuse.
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
            metalness: match kind {
                MaterialKind::Metal => amount("_metal"),
                // A blend's shares default to none of each.
                MaterialKind::Blend => self.float("_metal").unwrap_or(0.0).clamp(0.0, 1.0),
                _ => 0.0,
            },
            // Files from a recent version use `_trans`; the editor's own key table
            // (next to `_metal`, `_rough`, `_emit`...) names it `_alpha`.
            transparency: if kind == MaterialKind::Blend {
                self.float("_trans").or_else(|| self.float("_alpha")).unwrap_or(0.0).clamp(0.0, 1.0)
            } else if kind == MaterialKind::Glass {
                self.float("_trans")
                    .or_else(|| self.float("_alpha"))
                    .or(weight)
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            },
            emission: match kind {
                MaterialKind::Emissive => amount("_emit"),
                MaterialKind::Blend => self.float("_emit").unwrap_or(0.0).clamp(0.0, 1.0),
                _ => 0.0,
            },
            flux: self.float("_flux").unwrap_or(0.0).max(0.0),
            // Absorption density: `_d` in version 200, `_att` in version 150.
            // Zero means perfectly clear glass whatever its colour.
            density: self
                .float("_d")
                .or_else(|| self.float("_att"))
                .unwrap_or(0.0)
                .max(0.0),
            media: match self.float("_media").map(|v| v.round() as i32) {
                Some(1) => MediaKind::Scatter,
                Some(2) => MediaKind::Emit,
                Some(3) => MediaKind::Subsurface,
                _ => MediaKind::Absorb,
            },
            phase: self.float("_g").unwrap_or(0.0).clamp(-0.99, 0.99),
        }
    }
}

impl Material {
    /// The `MATL` entry for edited parameters, starting from the entry the
    /// file had so keys this editor does not model (`_spec`, `_ldr`,
    /// `_media`...) survive.
    ///
    /// Both conventions are written: `_weight` and `_att` for version 150
    /// readers, `_metal`, `_trans`, `_emit`, `_d` and `_ri` for version 200,
    /// so the file reads the same in either editor and in [`Material::resolve`].
    /// Keys of the other kinds are removed, or a glass turned metal would keep
    /// reading as glass in a reader that checks `_trans` first.
    pub fn from_resolved(id: i32, base: Option<&Material>, r: &ResolvedMaterial) -> Material {
        let mut d = base.map(|m| m.properties.clone()).unwrap_or_default();
        let kind = match r.kind {
            MaterialKind::Diffuse => "_diffuse",
            MaterialKind::Metal => "_metal",
            MaterialKind::Glass => "_glass",
            MaterialKind::Emissive => "_emit",
            MaterialKind::Media => "_media",
            MaterialKind::Blend => "_blend",
        };
        d.set("_type", kind);
        for key in ["_metal", "_trans", "_alpha", "_emit", "_weight"] {
            d.remove(key);
        }
        let number = |v: f32| format!("{}", (v * 1000.0).round() / 1000.0);
        d.set("_rough", number(r.roughness));
        d.set("_ior", number(r.ior - 1.0));
        d.set("_ri", number(r.ior));
        d.set("_flux", number(r.flux));
        d.set("_d", number(r.density));
        d.set("_att", number(r.density));
        match r.kind {
            MaterialKind::Diffuse => {}
            MaterialKind::Metal => {
                d.set("_metal", number(r.metalness));
                d.set("_weight", number(r.metalness));
            }
            MaterialKind::Glass => {
                d.set("_trans", number(r.transparency));
                d.set("_weight", number(r.transparency));
            }
            MaterialKind::Emissive => {
                d.set("_emit", number(r.emission));
                d.set("_weight", number(r.emission));
            }
            MaterialKind::Media => {
                d.set("_media", (r.media as u32).to_string());
                d.set("_g", number(r.phase));
            }
            // Every share under its own key, as version 200 writes them.
            MaterialKind::Blend => {
                d.set("_metal", number(r.metalness));
                d.set("_trans", number(r.transparency));
                d.set("_emit", number(r.emission));
            }
        }
        Material { id, properties: d }
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
    /// A participating medium: cloud, smoke, fog in a volume.
    Media = 4,
    /// the reference editor's Blend: shares of metal, glass and emission at once.
    Blend = 5,
}

/// How a medium interacts with light, as the editor's `_media` numbers it
/// (`the reference renderer` puts the density in a different slot
/// for each).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum MediaKind {
    /// Darkens what passes through, tinted by the colour.
    Absorb = 0,
    /// Scatters light, like a cloud.
    Scatter = 1,
    /// Glows with its colour.
    Emit = 2,
    /// Scatters, flagged by the editor as subsurface scattering.
    Subsurface = 3,
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
    /// How strongly a transparent material absorbs per voxel travelled; for
    /// a medium, its density.
    pub density: f32,
    /// For a medium: how it interacts with light (`_media`).
    pub media: MediaKind,
    /// For a medium: the Henyey-Greenstein anisotropy (`_g`), -1 back to 1
    /// forward scattering.
    pub phase: f32,
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
            media: MediaKind::Absorb,
            phase: 0.0,
        }
    }
}

/// `rCAM`: a saved camera. Only written by format version 200 and later.
#[derive(Debug, Clone, Default, PartialEq)]
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

    #[test]
    fn edited_materials_resolve_to_what_was_edited() {
        let cases = [
            ResolvedMaterial { kind: MaterialKind::Metal, metalness: 0.7, roughness: 0.3, ior: 1.5, ..Default::default() },
            ResolvedMaterial { kind: MaterialKind::Glass, transparency: 0.4, density: 0.2, ior: 1.8, ..Default::default() },
            ResolvedMaterial { kind: MaterialKind::Emissive, emission: 0.5, flux: 2.0, ..Default::default() },
            ResolvedMaterial { kind: MaterialKind::Diffuse, roughness: 0.6, ..Default::default() },
            ResolvedMaterial {
                kind: MaterialKind::Media,
                media: MediaKind::Scatter,
                density: 0.3,
                phase: 0.4,
                ..Default::default()
            },
        ];
        for r in cases {
            let m = Material::from_resolved(9, None, &r);
            assert_eq!(m.id, 9);
            assert_eq!(m.resolve(), r, "{:?}", m.properties);
        }
    }

    #[test]
    fn changing_kind_drops_the_old_kind_and_keeps_unknown_keys() {
        let glass = Material {
            id: 3,
            properties: Dict(vec![
                ("_type".into(), "_glass".into()),
                ("_trans".into(), "0.8".into()),
                ("_spec".into(), "0.5".into()),
            ]),
        };
        let metal = ResolvedMaterial { kind: MaterialKind::Metal, metalness: 1.0, ..Default::default() };
        let m = Material::from_resolved(3, Some(&glass), &metal);
        assert_eq!(m.properties.get("_trans"), None);
        assert_eq!(m.properties.get("_spec"), Some("0.5"));
        assert_eq!(m.resolve().kind, MaterialKind::Metal);
    }
}

