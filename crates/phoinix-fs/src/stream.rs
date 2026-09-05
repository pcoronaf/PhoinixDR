//! A logical stream over a list of byte extents of a volume, shared by
//! cluster-based filesystems (FAT, exFAT).

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::arith;

use crate::FsError;

/// One contiguous piece of a stream on the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    /// Volume byte offset.
    pub offset: u64,
    /// Length in bytes.
    pub length: u64,
}

/// Maps file offsets onto volume extents; bytes beyond the logical length
/// are never exposed.
#[derive(Debug, Clone)]
pub struct ExtentStream {
    reader: Arc<dyn BlockReader>,
    extents: Arc<Vec<Extent>>,
    len: u64,
}

impl ExtentStream {
    /// Creates a stream of `len` logical bytes over `extents`.
    #[must_use]
    pub fn new(reader: Arc<dyn BlockReader>, extents: Vec<Extent>, len: u64) -> Self {
        Self {
            reader,
            extents: Arc::new(extents),
            len,
        }
    }

    /// Logical length.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Whether the stream is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The extents.
    #[must_use]
    pub fn extents(&self) -> &[Extent] {
        &self.extents
    }

    /// Bytes covered by the extents.
    #[must_use]
    pub fn covered(&self) -> u64 {
        self.extents
            .iter()
            .fold(0u64, |a, e| a.saturating_add(e.length))
    }

    /// Reads at `offset`; returns 0 at or beyond the logical end.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Malformed`] if the extents do not cover the
    /// requested region, or a block error.
    pub fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, FsError> {
        if offset >= self.len || buffer.is_empty() {
            return Ok(0);
        }
        let want = usize::try_from((self.len - offset).min(buffer.len() as u64))
            .map_err(|_| FsError::Overflow)?;
        let buffer = buffer.get_mut(..want).ok_or(FsError::Overflow)?;
        let mut done = 0usize;
        let mut pos = offset;
        let mut logical = 0u64;
        let mut idx = 0usize;
        // Skip extents entirely before `pos`.
        while let Some(e) = self.extents.get(idx) {
            if arith::add(logical, e.length)? > pos {
                break;
            }
            logical = arith::add(logical, e.length)?;
            idx += 1;
        }
        while done < want {
            let Some(e) = self.extents.get(idx) else {
                return Err(FsError::Malformed {
                    structure: "extent list",
                    detail: format!("no extent covers byte {pos}"),
                });
            };
            let within = pos - logical;
            let chunk = usize::try_from((e.length - within).min((want - done) as u64))
                .map_err(|_| FsError::Overflow)?;
            let dst = buffer
                .get_mut(done..done + chunk)
                .ok_or(FsError::Overflow)?;
            self.reader
                .read_exact_at(arith::add(e.offset, within)?, dst)?;
            done += chunk;
            pos = arith::add(pos, chunk as u64)?;
            if pos >= arith::add(logical, e.length)? {
                logical = arith::add(logical, e.length)?;
                idx += 1;
            }
        }
        Ok(done)
    }

    /// Reads the whole stream, refusing streams larger than `limit`.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Unsupported`] above the limit or any read error.
    pub fn read_all(&self, limit: u64) -> Result<Vec<u8>, FsError> {
        if self.len > limit {
            return Err(FsError::Unsupported(format!(
                "stream of {} bytes exceeds the {limit}-byte limit",
                self.len
            )));
        }
        let mut out = vec![0u8; usize::try_from(self.len).map_err(|_| FsError::Overflow)?];
        let n = self.read_at(0, &mut out)?;
        if n as u64 != self.len {
            return Err(FsError::Malformed {
                structure: "extent list",
                detail: "short read".into(),
            });
        }
        Ok(out)
    }

    /// A `Read + Seek` cursor.
    #[must_use]
    pub fn cursor(&self) -> ExtentStreamCursor {
        ExtentStreamCursor {
            stream: self.clone(),
            pos: 0,
        }
    }
}

/// Cursor over an [`ExtentStream`].
#[derive(Debug, Clone)]
pub struct ExtentStreamCursor {
    stream: ExtentStream,
    pos: u64,
}

impl ExtentStreamCursor {
    /// The underlying stream.
    #[must_use]
    pub const fn stream(&self) -> &ExtentStream {
        &self.stream
    }
}

impl Read for ExtentStreamCursor {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self
            .stream
            .read_at(self.pos, buf)
            .map_err(|e| io::Error::other(e.to_string()))?;
        self.pos = self.pos.saturating_add(n as u64);
        Ok(n)
    }
}

impl Seek for ExtentStreamCursor {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new = match pos {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.stream.len().checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos =
            new.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of range"))?;
        Ok(self.pos)
    }
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

    use super::*;
    use phoinix_block::MemoryReader;

    #[test]
    fn reads_across_extents_and_stops_at_logical_end() {
        let data: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let reader: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(data.clone()));
        let s = ExtentStream::new(
            reader,
            vec![
                Extent {
                    offset: 1000,
                    length: 100,
                },
                Extent {
                    offset: 3000,
                    length: 100,
                },
                Extent {
                    offset: 0,
                    length: 100,
                },
            ],
            250,
        );
        let all = s.read_all(1 << 20).unwrap();
        assert_eq!(&all[..100], &data[1000..1100]);
        assert_eq!(&all[100..200], &data[3000..3100]);
        assert_eq!(&all[200..250], &data[0..50]);
        let mut buf = [0u8; 20];
        assert_eq!(s.read_at(95, &mut buf).unwrap(), 20);
        assert_eq!(&buf[..5], &data[1095..1100]);
        assert_eq!(&buf[5..], &data[3000..3015]);
        assert_eq!(s.read_at(250, &mut buf).unwrap(), 0);
        let mut c = s.cursor();
        c.seek(SeekFrom::End(-10)).unwrap();
        let mut tail = Vec::new();
        c.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, &data[40..50]);
        // Gap: extents cover only 200 bytes of a 300-byte stream.
        let short = ExtentStream::new(
            Arc::new(MemoryReader::new(data)),
            vec![Extent {
                offset: 0,
                length: 200,
            }],
            300,
        );
        assert!(short.read_all(1 << 20).is_err());
    }
}
