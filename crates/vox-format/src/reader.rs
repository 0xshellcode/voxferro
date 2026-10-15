//! Bounds-checked cursor over the file bytes.

use crate::VoxError;

type Result<T> = std::result::Result<T, VoxError>;

/// A cursor over the file bytes.
///
/// Holding a borrow plus an offset — rather than reslicing as we go — keeps a
/// single immutable borrow alive for the whole parse. Every read is bounds
/// checked and returns an error instead of panicking, so a truncated or
/// hostile file cannot take the process down.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    /// A reader over a sub-slice, used to parse one chunk's content in
    /// isolation so a malformed chunk cannot read into its neighbour.
    pub fn sub(&self, start: usize, len: usize) -> Result<Reader<'a>> {
        let end = start.checked_add(len).ok_or(VoxError::UnexpectedEof {
            offset: start,
            wanted: len,
            available: 0,
        })?;
        if end > self.data.len() {
            return Err(VoxError::UnexpectedEof {
                offset: start,
                wanted: len,
                available: self.data.len().saturating_sub(start),
            });
        }
        Ok(Reader {
            data: &self.data[start..end],
            pos: 0,
        })
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

    pub fn skip(&mut self, n: usize) -> Result<()> {
        self.take(n).map(|_| ())
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

    /// Reads a count field and rejects absurd values before they are used to
    /// reserve memory, so a corrupt length cannot trigger a huge allocation.
    pub fn count(&mut self, limit: u32, what: &'static str) -> Result<u32> {
        let n = self.u32()?;
        if n > limit {
            return Err(VoxError::ImplausibleCount {
                what,
                value: n,
                limit,
            });
        }
        Ok(n)
    }

    /// Reads a four-byte chunk id such as `SIZE` or `XYZI`.
    pub fn chunk_id(&mut self) -> Result<[u8; 4]> {
        let b = self.take(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }

    /// Reads a length-prefixed string.
    ///
    /// Decoded lossily: the format gives no encoding, and a stray byte in a
    /// model's name must not fail the whole load.
    pub fn string(&mut self) -> Result<String> {
        let len = self.count(crate::limits::MAX_STRING_LEN, "string length")? as usize;
        let bytes = self.take(len)?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}
