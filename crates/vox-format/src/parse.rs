//! Walks the chunk tree and collects everything into a [`VoxFile`].

use crate::{
    Camera, Color, Dict, Layer, Material, Model, Palette, Reader, SceneNode, ShapeModel,
    ShapeNode, GroupNode, TransformNode, VoxError, Voxel, limits,
};

type Result<T> = std::result::Result<T, VoxError>;

/// A chunk header: id, the length of its own content, and the length of its
/// children.
///
/// Honouring both lengths is what makes the format forward compatible: an
/// unknown chunk can be stepped over exactly, children and all.
#[derive(Debug, Clone, Copy)]
struct ChunkHeader {
    id: [u8; 4],
    content_len: u32,
    children_len: u32,
}

/// A chunk this crate does not interpret, recorded so callers can see what a
/// file holds that we ignore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownChunk {
    pub id: String,
    pub content_len: u32,
}

/// Everything a `.vox` file contains.
#[derive(Debug, Clone)]
pub struct VoxFile {
    pub version: u32,
    /// Models in file order; a shape node's `model_id` indexes this.
    pub models: Vec<Model>,
    pub palette: Palette,
    /// Scene graph nodes as stored. `voxel-core` turns these into a tree.
    pub nodes: Vec<SceneNode>,
    pub layers: Vec<Layer>,
    /// One per palette index that carries a `MATL` chunk.
    pub materials: Vec<Material>,
    /// `rOBJ` render settings: sun, sky, atmosphere, lens, film, bloom…
    pub render_objects: Vec<Dict>,
    /// `rCAM` saved cameras. Empty in version 150 files.
    pub cameras: Vec<Camera>,
    /// `NOTE` names for the 32 palette rows. Empty in version 150 files.
    pub color_names: Vec<String>,
    /// `IMAP` palette index remap.
    ///
    /// Optional, and only written by version 150: the editor emits it once the
    /// palette has been reordered. One file in the sample corpus has one.
    pub index_map: Option<Box<[u8; 256]>>,
    /// `META` key/values, such as `_anim_range` on animated models.
    pub meta: Vec<Dict>,
    pub unknown_chunks: Vec<UnknownChunk>,
}

impl VoxFile {
    /// A file holding one empty model of the given size, placed the way the
    /// editor places a new model: on the ground, at the origin.
    ///
    /// Built by writing and re-reading, so the scene graph it carries is
    /// exactly the one a save will write -- a new model and its saved copy
    /// cannot disagree about where it sits.
    pub fn new_single(size: [u32; 3]) -> Self {
        let size = size.map(|s| s.clamp(1, 256));
        let file = Self {
            version: crate::VERSION_200,
            models: vec![Model { size, voxels: Vec::new() }],
            palette: Palette::specification_default(),
            nodes: Vec::new(),
            layers: Vec::new(),
            materials: Vec::new(),
            render_objects: Vec::new(),
            cameras: Vec::new(),
            color_names: Vec::new(),
            index_map: None,
            meta: Vec::new(),
            unknown_chunks: Vec::new(),
        };
        Self::parse(&file.to_bytes()).unwrap_or(file)
    }

    /// Parses a whole `.vox` file.
    /// *Recover*: the models and palette of a damaged file,
    /// and nothing else. The chunk tree isn't trusted: every `SIZE` chunk
    /// followed by an `XYZI` is taken wherever it sits, a truncated `XYZI`
    /// keeps the voxels that are there, voxels outside their box are
    /// dropped, and the last whole `RGBA` is the palette. `None` when no
    /// model is found.
    pub fn recover(data: &[u8]) -> Option<Self> {
        let u32_at = |i: usize| -> Option<u32> { Some(u32::from_le_bytes(data.get(i..i + 4)?.try_into().ok()?)) };
        let mut models = Vec::new();
        let mut palette = Palette::specification_default();
        let mut i = 0usize;
        while i + 12 <= data.len() {
            match &data[i..i + 4] {
                b"SIZE" if u32_at(i + 4) == Some(12) => {
                    let size = [u32_at(i + 12), u32_at(i + 16), u32_at(i + 20)];
                    let next = i + 24;
                    if let [Some(x), Some(y), Some(z)] = size
                        && [x, y, z].iter().all(|s| (1..=256).contains(s))
                        && data.get(next..next + 4) == Some(b"XYZI")
                        && let Some(n) = u32_at(next + 12)
                    {
                        let start = next + 16;
                        let available = data.len().saturating_sub(start) / 4;
                        let voxels = data[start..start + 4 * (n as usize).min(available)]
                            .as_chunks::<4>().0.iter()
                            .filter(|v| (v[0] as u32) < x && (v[1] as u32) < y && (v[2] as u32) < z && v[3] != 0)
                            .map(|v| Voxel { x: v[0], y: v[1], z: v[2], index: v[3] })
                            .collect();
                        models.push(Model { size: [x, y, z], voxels });
                        i = start;
                        continue;
                    }
                }
                b"RGBA" if u32_at(i + 4) == Some(1024) && data.len() >= i + 12 + 1024 => {
                    let mut entries = [Color::default(); 256];
                    for (k, slot) in entries.iter_mut().enumerate() {
                        let c = &data[i + 12 + 4 * k..i + 16 + 4 * k];
                        *slot = Color { r: c[0], g: c[1], b: c[2], a: c[3] };
                    }
                    palette = Palette::from_chunk(&entries);
                }
                _ => {}
            }
            i += 1;
        }
        if models.is_empty() {
            return None;
        }
        let mut file = Self::new_single([1, 1, 1]);
        file.version = crate::VERSION_150;
        file.models = models;
        file.palette = palette;
        file.nodes.clear();
        file.layers.clear();
        Some(file)
    }

    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);

        let magic = r.chunk_id()?;
        if &magic != crate::MAGIC {
            return Err(VoxError::BadMagic { found: magic });
        }
        let version = r.u32()?;

        let mut out = Self {
            version,
            models: Vec::new(),
            palette: Palette::default(),
            nodes: Vec::new(),
            layers: Vec::new(),
            materials: Vec::new(),
            render_objects: Vec::new(),
            cameras: Vec::new(),
            color_names: Vec::new(),
            index_map: None,
            meta: Vec::new(),
            unknown_chunks: Vec::new(),
        };

        // SIZE precedes the XYZI it describes; hold it until the pair closes.
        let mut pending_size: Option<[u32; 3]> = None;
        out.walk(&mut r, 0, &mut pending_size)?;

        if let Some(size) = pending_size {
            return Err(VoxError::DanglingSize { size });
        }
        Ok(out)
    }

    fn walk(
        &mut self,
        r: &mut Reader<'_>,
        depth: u32,
        pending_size: &mut Option<[u32; 3]>,
    ) -> Result<()> {
        if depth > limits::MAX_CHUNK_DEPTH {
            return Err(VoxError::TooDeep {
                limit: limits::MAX_CHUNK_DEPTH,
            });
        }

        while !r.is_empty() {
            // A trailing run shorter than a header means a truncated file.
            if r.remaining() < 12 {
                return Err(VoxError::UnexpectedEof {
                    offset: r.position(),
                    wanted: 12,
                    available: r.remaining(),
                });
            }
            let header = ChunkHeader {
                id: r.chunk_id()?,
                content_len: r.u32()?,
                children_len: r.u32()?,
            };

            let content_start = r.position();
            let mut content = r.sub(content_start, header.content_len as usize)?;
            r.skip(header.content_len as usize)?;

            let children_start = r.position();
            let mut children = r.sub(children_start, header.children_len as usize)?;
            r.skip(header.children_len as usize)?;

            self.chunk(&header, &mut content, pending_size)?;

            if !children.is_empty() {
                self.walk(&mut children, depth + 1, pending_size)?;
            }
        }
        Ok(())
    }

    fn chunk(
        &mut self,
        header: &ChunkHeader,
        c: &mut Reader<'_>,
        pending_size: &mut Option<[u32; 3]>,
    ) -> Result<()> {
        match &header.id {
            b"MAIN" => {} // A pure container; its payload is its children.

            b"SIZE" => {
                let size = [c.u32()?, c.u32()?, c.u32()?];
                if size.iter().any(|&s| s == 0 || s > limits::MAX_MODEL_SIDE) {
                    return Err(VoxError::ImplausibleSize { size });
                }
                if pending_size.replace(size).is_some() {
                    return Err(VoxError::DanglingSize { size });
                }
            }

            b"XYZI" => {
                let size = pending_size.take().ok_or(VoxError::XyziWithoutSize)?;
                let n = c.count(limits::MAX_VOXELS, "voxel count")?;
                let mut voxels = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    voxels.push(Voxel {
                        x: c.u8()?,
                        y: c.u8()?,
                        z: c.u8()?,
                        index: c.u8()?,
                    });
                }
                self.models.push(Model { size, voxels });
            }

            b"RGBA" => {
                let mut entries = [Color::default(); 256];
                for slot in entries.iter_mut() {
                    *slot = Color {
                        r: c.u8()?,
                        g: c.u8()?,
                        b: c.u8()?,
                        a: c.u8()?,
                    };
                }
                self.palette = Palette::from_chunk(&entries);
            }

            b"nTRN" => {
                let id = c.i32()?;
                let attributes = Dict::parse(c)?;
                let child = c.i32()?;
                let _reserved = c.i32()?; // always -1
                let layer = c.i32()?;
                let n = c.count(limits::MAX_FRAMES, "frame count")?;
                let mut frames = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    frames.push(Dict::parse(c)?);
                }
                self.nodes.push(SceneNode::Transform(TransformNode {
                    id,
                    attributes,
                    child,
                    layer,
                    frames,
                }));
            }

            b"nGRP" => {
                let id = c.i32()?;
                let attributes = Dict::parse(c)?;
                let n = c.count(limits::MAX_NODE_CHILDREN, "group children")?;
                let mut children = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    children.push(c.i32()?);
                }
                self.nodes.push(SceneNode::Group(GroupNode {
                    id,
                    attributes,
                    children,
                }));
            }

            b"nSHP" => {
                let id = c.i32()?;
                let attributes = Dict::parse(c)?;
                let n = c.count(limits::MAX_SHAPE_MODELS, "shape models")?;
                let mut models = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    models.push(ShapeModel {
                        model_id: c.i32()?,
                        attributes: Dict::parse(c)?,
                    });
                }
                self.nodes.push(SceneNode::Shape(ShapeNode {
                    id,
                    attributes,
                    models,
                }));
            }

            b"LAYR" => {
                let id = c.i32()?;
                let attributes = Dict::parse(c)?;
                let _reserved = c.i32()?; // always -1
                self.layers.push(Layer { id, attributes });
            }

            b"MATL" => {
                let id = c.i32()?;
                let properties = Dict::parse(c)?;
                self.materials.push(Material { id, properties });
            }

            b"rOBJ" => self.render_objects.push(Dict::parse(c)?),

            b"rCAM" => {
                let id = c.i32()?;
                let attributes = Dict::parse(c)?;
                self.cameras.push(Camera { id, attributes });
            }

            b"NOTE" => {
                let n = c.count(limits::MAX_COLOR_NAMES, "colour names")?;
                let mut names = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    names.push(c.string()?);
                }
                self.color_names = names;
            }

            b"IMAP" => {
                let bytes = c.take(256)?;
                let mut map = Box::new([0u8; 256]);
                map.copy_from_slice(bytes);
                self.index_map = Some(map);
            }

            b"META" => self.meta.push(Dict::parse(c)?),

            // Unknown chunk: recorded and stepped over. The caller already
            // advanced past both lengths, so nothing is lost.
            other => self.unknown_chunks.push(UnknownChunk {
                id: String::from_utf8_lossy(other).into_owned(),
                content_len: header.content_len,
            }),
        }
        Ok(())
    }

    /// Total voxels across every model.
    pub fn voxel_count(&self) -> usize {
        self.models.iter().map(|m| m.voxels.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_over_256_a_side_is_refused_before_any_allocation() {
        // A fuzzed file declared 20 x 2147483647 x 20: meshing it never ended.
        let mut file = VoxFile::new_single([4, 4, 4]);
        let ok = file.to_bytes();
        assert!(VoxFile::parse(&ok).is_ok());
        for size in [[20, 0x7FFF_FFFF, 20], [257, 1, 1], [0, 4, 4]] {
            file.models[0].size = size;
            let err = VoxFile::parse(&file.to_bytes()).unwrap_err();
            assert!(matches!(err, VoxError::ImplausibleSize { .. }), "{size:?}: {err}");
        }
    }

    #[test]
    fn recover_reads_models_from_a_broken_file() {
        let mut file = VoxFile::new_single([4, 4, 4]);
        file.models[0].voxels.push(Voxel { x: 1, y: 2, z: 3, index: 9 });
        file.models[0].voxels.push(Voxel { x: 0, y: 0, z: 0, index: 5 });
        let mut bytes = file.to_bytes();
        // Break the header and cut the file short of its scene graph.
        bytes[0] = b'X';
        let xyzi = bytes.windows(4).position(|w| w == b"XYZI").unwrap();
        bytes.truncate(xyzi + 16 + 8);
        assert!(VoxFile::parse(&bytes).is_err());
        let back = VoxFile::recover(&bytes).unwrap();
        assert_eq!(back.models.len(), 1);
        assert_eq!(back.models[0].size, [4, 4, 4]);
        assert_eq!(back.models[0].voxels.len(), 2);
        assert!(VoxFile::recover(b"nothing here at all").is_none());
    }
}
