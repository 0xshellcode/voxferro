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
