//! Reader for the reference editor `.vox` files.
//!
//! The format is a RIFF-like container: a `VOX ` magic, a `u32` version, then
//! a tree of chunks. Every chunk header is three fields — a four-byte id, the
//! byte length of its own content, and the byte length of its children — so a
//! reader that honours both lengths can step over any chunk it does not know.
//! That rule is why one parser handles every version.
//!
//! ```no_run
//! let bytes = std::fs::read("model.vox")?;
//! let file = vox_format::VoxFile::parse(&bytes)?;
//! println!("{} models, {} voxels", file.models.len(), file.voxel_count());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! Layouts were verified byte by byte against the sample models shipped with
//! the reference editor a recent version and a recent version.

mod dict;
mod parse;
mod reader;
mod types;

pub use dict::Dict;
pub use parse::{UnknownChunk, VoxFile};
pub use reader::Reader;
pub use types::{
    Camera, Color, GroupNode, Layer, Material, Model, Palette, SceneNode, ShapeModel, ShapeNode,
    TransformNode, Voxel,
};

use thiserror::Error;

/// Magic bytes that open every `.vox` file.
pub const MAGIC: &[u8; 4] = b"VOX ";

/// Version written by the reference editor up to a recent version.x.
pub const VERSION_150: u32 = 150;

/// Version written by the reference editor a recent version.x.
///
/// Adds the `rCAM` (saved cameras), `NOTE` (palette row names) and `META`
/// chunks, and stops writing `IMAP`. The chunk-skipping rule means a reader
/// that handles 150 also loads 200 files; only the new chunks need explicit
/// support.
pub const VERSION_200: u32 = 200;

/// Versions this crate claims to read.
pub const SUPPORTED_VERSIONS: &[u32] = &[VERSION_150, VERSION_200];

/// Whether `version` is one this crate claims to support.
pub fn is_supported_version(version: u32) -> bool {
    SUPPORTED_VERSIONS.contains(&version)
}

/// Ceilings applied to every count read from a file.
///
/// A corrupt or hostile length field must not be handed straight to
/// `Vec::with_capacity`. These bounds sit far above anything the editor
/// produces, so they reject nonsense without rejecting real files.
pub mod limits {
    /// 256³, the largest volume the editor allows, fully solid.
    pub const MAX_VOXELS: u32 = 256 * 256 * 256;
    pub const MAX_DICT_PAIRS: u32 = 4096;
    pub const MAX_STRING_LEN: u32 = 1 << 20;
    pub const MAX_FRAMES: u32 = 65536;
    pub const MAX_NODE_CHILDREN: u32 = 1 << 20;
    pub const MAX_SHAPE_MODELS: u32 = 65536;
    pub const MAX_COLOR_NAMES: u32 = 1024;
    pub const MAX_CHUNK_DEPTH: u32 = 32;
}

#[derive(Debug, Error)]
pub enum VoxError {
    #[error("not a vox file: expected magic {:?}, found {found:?}", MAGIC)]
    BadMagic { found: [u8; 4] },

    #[error("unexpected end of file: wanted {wanted} bytes at offset {offset}, {available} available")]
    UnexpectedEof {
        offset: usize,
        wanted: usize,
        available: usize,
    },

    #[error("implausible {what}: {value} exceeds the limit of {limit}")]
    ImplausibleCount {
        what: &'static str,
        value: u32,
        limit: u32,
    },

    #[error("XYZI chunk without a preceding SIZE chunk")]
    XyziWithoutSize,

    #[error("SIZE chunk {size:?} was never followed by an XYZI chunk")]
    DanglingSize { size: [u32; 3] },

    #[error("chunk nesting deeper than {limit} levels")]
    TooDeep { limit: u32 },
}

pub type Result<T> = std::result::Result<T, VoxError>;

/// Validates the magic and returns the file's version without parsing chunks.
pub fn peek_version(data: &[u8]) -> Result<u32> {
    let mut r = Reader::new(data);
    let magic = r.chunk_id()?;
    if &magic != MAGIC {
        return Err(VoxError::BadMagic { found: magic });
    }
    r.u32()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_vox_data() {
        let err = peek_version(b"NOPE\x00\x00\x00\x00").unwrap_err();
        assert!(matches!(err, VoxError::BadMagic { .. }));
    }

    #[test]
    fn rejects_truncated_header() {
        let err = peek_version(b"VOX ").unwrap_err();
        assert!(matches!(err, VoxError::UnexpectedEof { .. }));
    }

    #[test]
    fn known_versions_are_supported() {
        assert!(is_supported_version(VERSION_150));
        assert!(is_supported_version(VERSION_200));
        assert!(!is_supported_version(99));
    }

    #[test]
    fn reader_reads_little_endian() {
        let mut r = Reader::new(&[0x96, 0x00, 0x00, 0x00]);
        assert_eq!(r.u32().unwrap(), 150);
        assert!(r.is_empty());
    }

    #[test]
    fn rejects_implausible_counts() {
        // A count field claiming four billion entries must not reach
        // Vec::with_capacity.
        let mut r = Reader::new(&[0xff, 0xff, 0xff, 0xff]);
        let err = r.count(limits::MAX_DICT_PAIRS, "dict pairs").unwrap_err();
        assert!(matches!(err, VoxError::ImplausibleCount { .. }));
    }

    #[test]
    fn palette_undoes_the_chunk_shift() {
        let mut entries = [Color::default(); 256];
        entries[0] = Color { r: 1, g: 2, b: 3, a: 255 };
        let pal = Palette::from_chunk(&entries);
        // Chunk entry 0 is the colour of voxel index 1.
        assert_eq!(pal.color(1), Color { r: 1, g: 2, b: 3, a: 255 });
        // Index 0 is empty and never carries a colour.
        assert_eq!(pal.color(0).a, 0);
    }

    #[test]
    fn dict_parses_numeric_values() {
        let d = Dict(vec![("_t".into(), "0 0 30".into())]);
        assert_eq!(d.get_parsed::<i32, 3>("_t"), Some([0, 0, 30]));
        assert_eq!(d.get_parsed::<i32, 3>("_missing"), None);
    }
}
