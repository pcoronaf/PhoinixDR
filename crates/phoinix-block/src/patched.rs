//! A read-only overlay: a reader whose bytes at given offsets are replaced
//! by in-memory patches. Used to mount a volume whose primary structure is
//! destroyed by substituting its backup, without writing to the source.

use std::sync::Arc;

use phoinix_core::{ByteRange, SourceId};

use crate::{BlockError, BlockGeometry, BlockReader};

/// One patch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// Offset of the first replaced byte.
    pub offset: u64,
    /// Replacement bytes.
    pub bytes: Vec<u8>,
}

/// A reader with in-memory patches over another reader.
#[derive(Debug)]
pub struct PatchedReader {
    id: SourceId,
    inner: Arc<dyn BlockReader>,
    patches: Vec<Patch>,
}

impl PatchedReader {
    /// Overlays `patches` on `inner`. Patches must lie inside the reader.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::OutOfBounds`] for a patch outside the reader.
    pub fn new(inner: Arc<dyn BlockReader>, patches: Vec<Patch>) -> Result<Self, BlockError> {
        for p in &patches {
            let len = p.bytes.len() as u64;
            if p.offset
                .checked_add(len)
                .is_none_or(|end| end > inner.len())
            {
                return Err(BlockError::OutOfBounds {
                    offset: p.offset,
                    length: len,
                    source_len: inner.len(),
                });
            }
        }
        Ok(Self {
            id: SourceId::new(),
            inner,
            patches,
        })
    }

    /// The patches.
    #[must_use]
    pub fn patches(&self) -> &[Patch] {
        &self.patches
    }
}

impl BlockReader for PatchedReader {
    fn id(&self) -> SourceId {
        self.id
    }

    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn geometry(&self) -> &BlockGeometry {
        self.inner.geometry()
    }

    fn describe(&self) -> String {
        format!(
            "{} (+{} patch(es))",
            self.inner.describe(),
            self.patches.len()
        )
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        let n = self.inner.read_at(offset, buffer)?;
        let read = ByteRange {
            offset,
            length: n as u64,
        };
        for p in &self.patches {
            let patch = ByteRange {
                offset: p.offset,
                length: p.bytes.len() as u64,
            };
            let start = read.offset.max(patch.offset);
            let end = read.end().min(patch.end());
            if start >= end {
                continue;
            }
            let dst = usize::try_from(start - read.offset).unwrap_or(usize::MAX);
            let src = usize::try_from(start - patch.offset).unwrap_or(usize::MAX);
            let len = usize::try_from(end - start).unwrap_or(0);
            if let (Some(d), Some(s)) = (
                buffer.get_mut(dst..dst.saturating_add(len)),
                p.bytes.get(src..src.saturating_add(len)),
            ) {
                d.copy_from_slice(s);
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;
    use crate::{BlockReaderExt, MemoryReader};

    #[test]
    fn patches_overlay_reads() {
        let inner: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(vec![0u8; 4096]));
        let r = PatchedReader::new(
            inner,
            vec![Patch {
                offset: 512,
                bytes: vec![7u8; 512],
            }],
        )
        .unwrap();
        assert_eq!(r.read_vec(0, 512).unwrap(), vec![0u8; 512]);
        assert_eq!(r.read_vec(512, 512).unwrap(), vec![7u8; 512]);
        let mixed = r.read_vec(500, 24).unwrap();
        assert_eq!(&mixed[..12], &[0u8; 12]);
        assert_eq!(&mixed[12..], &[7u8; 12]);
        let tail = r.read_vec(1000, 100).unwrap();
        assert_eq!(&tail[..24], &[7u8; 24]);
        assert_eq!(&tail[24..], &[0u8; 76]);
        assert!(
            PatchedReader::new(
                Arc::new(MemoryReader::new(vec![0u8; 100])),
                vec![Patch {
                    offset: 90,
                    bytes: vec![1; 20]
                }]
            )
            .is_err()
        );
    }
}
