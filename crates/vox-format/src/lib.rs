//! Reader and writer for the reference editor `.vox` files.
//!
//! The format is a RIFF-like container: a `VOX ` magic, a `u32` version, then
//! a tree of chunks. Every chunk header is three fields: a four-byte id, the
//! byte length of its own content, and the byte length of its children. A
//! reader that honours both lengths can skip any chunk it does not know, which
//! is what keeps the format forward compatible.
//!
//! Phase 1 fills in the chunk tree. For now this covers the header and the
//! byte cursor every later stage builds on.

use thiserror::Error;

/// Magic bytes that open every `.vox` file.
pub const MAGIC: &[u8; 4] = b"VOX ";

/// Version written by the reference editor up to a recent version.x.
pub const VERSION_150: u32 = 150;

/// Version written by the reference editor a recent version.x.
///
/// Adds the `rCAM` (saved cameras), `NOTE` (palette notes) and `META` chunks,
/// and drops `IMAP`. The chunk-skipping rule means a reader that handles 150
/// also loads 200 files; only the new chunks need explicit support.
pub const VERSION_200: u32 = 200;

/// Versions this crate knows how to read.
pub const SUPPORTED_VERSIONS: &[u32] = &[VERSION_150, VERSION_200];

/// Whether `version` is one this crate claims to support.
pub fn is_supported_version(version: u32) -> bool {
    SUPPORTED_VERSIONS.contains(&version)
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
}

pub type Result<T> = std::result::Result<T, VoxError>;

/// A cursor over the file bytes.
///
/// Holding a borrow plus an offset — rather than reslicing as we go — keeps a
/// single immutable borrow alive for the whole parse. Every read is bounds
/// checked and returns an error instead of panicking, so a truncated or hostile
/// file cannot take the process down.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(VoxError::UnexpectedEof {
                offset: self.pos,
                wanted: n,
                available: self.remaining(),
            });
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    /// Reads a four-byte chunk id such as `SIZE` or `XYZI`.
    pub fn chunk_id(&mut self) -> Result<[u8; 4]> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }
}

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
}
