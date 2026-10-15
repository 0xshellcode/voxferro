//! The `DICT` structure: the format's universal key/value bag.
//!
//! Scene nodes, layers, materials, cameras and render settings all carry their
//! payload as string pairs rather than packed structs. That is why the format
//! survived six versions without breaking: a reader that does not recognise a
//! key just ignores it.

use crate::{Reader, VoxError, limits};

/// Key/value pairs, kept in file order.
///
/// A `Vec` rather than a `HashMap`: dictionaries are tiny (under a dozen
/// entries), lookup order is irrelevant at this size, and preserving file
/// order means a re-written file stays byte-comparable with the original.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dict(pub Vec<(String, String)>);

impl Dict {
    pub fn parse(r: &mut Reader<'_>) -> Result<Self, VoxError> {
        let n = r.count(limits::MAX_DICT_PAIRS, "dict pairs")?;
        let mut pairs = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let key = r.string()?;
            let value = r.string()?;
            pairs.push((key, value));
        }
        Ok(Self(pairs))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// Sets a key, replacing its value in place if present so file order
    /// is kept, appending it otherwise.
    pub fn set(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.0.retain(|(k, _)| k != key);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Parses a value written as whitespace-separated numbers, such as the
    /// `"0 0 30"` of a translation or the `"255 255 204"` of a colour.
    pub fn get_parsed<T: std::str::FromStr, const N: usize>(&self, key: &str) -> Option<[T; N]> {
        let raw = self.get(key)?;
        let mut parts = raw.split_whitespace();
        let mut out = Vec::with_capacity(N);
        for _ in 0..N {
            out.push(parts.next()?.parse().ok()?);
        }
        out.try_into().ok()
    }
}
