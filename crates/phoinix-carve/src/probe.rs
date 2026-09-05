//! A cached random-access view over a block reader, bounded by a limit.
//!
//! Assemblers walk file structures forward from a header; the probe keeps a
//! small window so that a chain of small reads (chunk headers, markers)
//! costs one block read per window instead of one I/O per field.

use phoinix_block::BlockReader;

use crate::CarveError;

/// Bytes kept in the window.
pub const WINDOW_BYTES: usize = 256 * 1024;

/// A bounded, cached reader.
pub struct Probe<'a> {
    reader: &'a dyn BlockReader,
    limit: u64,
    window_offset: u64,
    window: Vec<u8>,
}

impl<'a> Probe<'a> {
    /// A probe over `reader`, refusing reads at or beyond `limit`
    /// (typically the volume length).
    #[must_use]
    pub fn new(reader: &'a dyn BlockReader, limit: u64) -> Self {
        Self {
            reader,
            limit: limit.min(reader.len()),
            window_offset: 0,
            window: Vec::new(),
        }
    }

    /// Exclusive end of the readable region.
    #[must_use]
    pub const fn limit(&self) -> u64 {
        self.limit
    }

    /// Loads the window so that it starts at `offset`.
    fn load(&mut self, offset: u64) -> Result<(), CarveError> {
        let remaining = self.limit.saturating_sub(offset);
        let len = usize::try_from(remaining.min(WINDOW_BYTES as u64)).unwrap_or(WINDOW_BYTES);
        self.window.resize(len, 0);
        let mut filled = 0usize;
        while filled < len {
            let Some(tail) = self.window.get_mut(filled..) else {
                break;
            };
            let n = self
                .reader
                .read_at(offset.saturating_add(filled as u64), tail)?;
            if n == 0 {
                break;
            }
            filled = filled.saturating_add(n);
        }
        self.window.truncate(filled);
        self.window_offset = offset;
        Ok(())
    }

    /// Reads exactly `len` bytes at `offset`.
    ///
    /// # Errors
    ///
    /// Returns [`CarveError::Truncated`] if the read would cross the limit.
    pub fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, CarveError> {
        let end = offset.saturating_add(len as u64);
        if end > self.limit || offset > self.limit {
            return Err(CarveError::Truncated {
                offset,
                length: len as u64,
                limit: self.limit,
            });
        }
        if len == 0 {
            return Ok(Vec::new());
        }
        if len > WINDOW_BYTES {
            let mut buf = vec![0u8; len];
            let mut filled = 0usize;
            while filled < len {
                let Some(tail) = buf.get_mut(filled..) else {
                    break;
                };
                let n = self
                    .reader
                    .read_at(offset.saturating_add(filled as u64), tail)?;
                if n == 0 {
                    break;
                }
                filled = filled.saturating_add(n);
            }
            if filled < len {
                return Err(CarveError::Truncated {
                    offset,
                    length: len as u64,
                    limit: self.limit,
                });
            }
            return Ok(buf);
        }
        let in_window = offset >= self.window_offset
            && end <= self.window_offset.saturating_add(self.window.len() as u64);
        if !in_window {
            self.load(offset)?;
        }
        let start =
            usize::try_from(offset.saturating_sub(self.window_offset)).unwrap_or(usize::MAX);
        self.window
            .get(start..start.saturating_add(len))
            .map(<[u8]>::to_vec)
            .ok_or(CarveError::Truncated {
                offset,
                length: len as u64,
                limit: self.limit,
            })
    }

    /// Reads up to `len` bytes at `offset`, shorter at the limit.
    ///
    /// # Errors
    ///
    /// Propagates block errors.
    pub fn read_available(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, CarveError> {
        let available = self.limit.saturating_sub(offset);
        let len = usize::try_from(available.min(len as u64)).unwrap_or(len);
        if len == 0 {
            return Ok(Vec::new());
        }
        self.read(offset, len)
    }

    /// One byte.
    ///
    /// # Errors
    ///
    /// See [`read`](Self::read).
    pub fn byte(&mut self, offset: u64) -> Result<u8, CarveError> {
        Ok(self.read(offset, 1)?.first().copied().unwrap_or(0))
    }

    /// Little-endian `u16`.
    ///
    /// # Errors
    ///
    /// See [`read`](Self::read).
    pub fn u16_le(&mut self, offset: u64) -> Result<u16, CarveError> {
        let b = self.read(offset, 2)?;
        Ok(u16::from_le_bytes([at(&b, 0), at(&b, 1)]))
    }

    /// Big-endian `u16`.
    ///
    /// # Errors
    ///
    /// See [`read`](Self::read).
    pub fn u16_be(&mut self, offset: u64) -> Result<u16, CarveError> {
        let b = self.read(offset, 2)?;
        Ok(u16::from_be_bytes([at(&b, 0), at(&b, 1)]))
    }

    /// Little-endian `u32`.
    ///
    /// # Errors
    ///
    /// See [`read`](Self::read).
    pub fn u32_le(&mut self, offset: u64) -> Result<u32, CarveError> {
        let b = self.read(offset, 4)?;
        Ok(u32::from_le_bytes([
            at(&b, 0),
            at(&b, 1),
            at(&b, 2),
            at(&b, 3),
        ]))
    }

    /// Big-endian `u32`.
    ///
    /// # Errors
    ///
    /// See [`read`](Self::read).
    pub fn u32_be(&mut self, offset: u64) -> Result<u32, CarveError> {
        let b = self.read(offset, 4)?;
        Ok(u32::from_be_bytes([
            at(&b, 0),
            at(&b, 1),
            at(&b, 2),
            at(&b, 3),
        ]))
    }

    /// Little-endian `u64`.
    ///
    /// # Errors
    ///
    /// See [`read`](Self::read).
    pub fn u64_le(&mut self, offset: u64) -> Result<u64, CarveError> {
        let b = self.read(offset, 8)?;
        let mut a = [0u8; 8];
        for (i, slot) in a.iter_mut().enumerate() {
            *slot = at(&b, i);
        }
        Ok(u64::from_le_bytes(a))
    }

    /// Big-endian `u64`.
    ///
    /// # Errors
    ///
    /// See [`read`](Self::read).
    pub fn u64_be(&mut self, offset: u64) -> Result<u64, CarveError> {
        let b = self.read(offset, 8)?;
        let mut a = [0u8; 8];
        for (i, slot) in a.iter_mut().enumerate() {
            *slot = at(&b, i);
        }
        Ok(u64::from_be_bytes(a))
    }

    /// Finds the first occurrence of `pattern` in `[from, to)`, clamped to
    /// the limit. Returns the offset of the match.
    ///
    /// # Errors
    ///
    /// Propagates block errors.
    pub fn find(&mut self, pattern: &[u8], from: u64, to: u64) -> Result<Option<u64>, CarveError> {
        if pattern.is_empty() {
            return Ok(None);
        }
        let to = to.min(self.limit);
        let mut pos = from;
        let overlap = pattern.len().saturating_sub(1);
        while pos < to {
            let want = usize::try_from((to - pos).min(WINDOW_BYTES as u64)).unwrap_or(WINDOW_BYTES);
            let buf = self.read_available(pos, want)?;
            if buf.len() < pattern.len() {
                break;
            }
            if let Some(i) = find_in(&buf, pattern) {
                return Ok(Some(pos.saturating_add(i as u64)));
            }
            let advance = buf.len().saturating_sub(overlap).max(1);
            pos = pos.saturating_add(advance as u64);
        }
        Ok(None)
    }
}

fn at(b: &[u8], i: usize) -> u8 {
    b.get(i).copied().unwrap_or(0)
}

/// First occurrence of `pattern` in `haystack`.
#[must_use]
pub fn find_in(haystack: &[u8], pattern: &[u8]) -> Option<usize> {
    if pattern.is_empty() || haystack.len() < pattern.len() {
        return None;
    }
    let first = *pattern.first()?;
    let last_start = haystack.len() - pattern.len();
    let mut i = 0usize;
    while i <= last_start {
        let p = haystack.get(i..)?.iter().position(|b| *b == first)?;
        let cand = i + p;
        if cand > last_start {
            return None;
        }
        if haystack.get(cand..cand + pattern.len()) == Some(pattern) {
            return Some(cand);
        }
        i = cand + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]
    use phoinix_block::MemoryReader;

    use super::*;

    #[test]
    fn reads_and_finds_across_windows() {
        let mut data = vec![0u8; WINDOW_BYTES * 2 + 100];
        data[WINDOW_BYTES - 1] = b'X';
        data[WINDOW_BYTES] = b'Y';
        data[WINDOW_BYTES * 2 + 50] = b'Z';
        let reader = MemoryReader::new(data.clone());
        let mut p = Probe::new(&reader, data.len() as u64);
        assert_eq!(
            p.find(b"XY", 0, data.len() as u64).unwrap(),
            Some(WINDOW_BYTES as u64 - 1)
        );
        assert_eq!(
            p.find(b"Z", 0, data.len() as u64).unwrap(),
            Some(WINDOW_BYTES as u64 * 2 + 50)
        );
        assert_eq!(p.find(b"Q", 0, data.len() as u64).unwrap(), None);
        assert_eq!(p.byte(WINDOW_BYTES as u64).unwrap(), b'Y');
        assert!(p.read(data.len() as u64 - 1, 2).is_err());
        assert_eq!(p.read_available(data.len() as u64 - 1, 2).unwrap().len(), 1);
        assert_eq!(
            p.read(10, WINDOW_BYTES + 5).unwrap().len(),
            WINDOW_BYTES + 5
        );
    }

    #[test]
    fn integer_helpers() {
        let reader = MemoryReader::new(vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let mut p = Probe::new(&reader, 8);
        assert_eq!(p.u16_le(0).unwrap(), 0x0201);
        assert_eq!(p.u16_be(0).unwrap(), 0x0102);
        assert_eq!(p.u32_le(0).unwrap(), 0x0403_0201);
        assert_eq!(p.u32_be(0).unwrap(), 0x0102_0304);
        assert_eq!(p.u64_be(0).unwrap(), 0x0102_0304_0506_0708);
        assert_eq!(p.u64_le(0).unwrap(), 0x0807_0605_0403_0201);
        assert_eq!(find_in(b"abcabd", b"abd"), Some(3));
        assert_eq!(find_in(b"abc", b"abcd"), None);
    }
}
