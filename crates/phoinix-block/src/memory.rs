//! In-memory block reader, mainly for tests and fixtures.

use phoinix_core::SourceId;

use crate::{BlockError, BlockGeometry, BlockReader, check_request};

/// A [`BlockReader`] backed by a byte vector.
#[derive(Debug, Clone)]
pub struct MemoryReader {
    id: SourceId,
    data: Vec<u8>,
    geometry: BlockGeometry,
}

impl MemoryReader {
    /// Wraps `data` with 512-byte sector geometry.
    #[must_use]
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            id: SourceId::new(),
            data,
            geometry: BlockGeometry::SECTOR_512,
        }
    }

    /// Wraps `data` with an explicit geometry.
    #[must_use]
    pub fn with_geometry(data: Vec<u8>, geometry: BlockGeometry) -> Self {
        Self {
            id: SourceId::new(),
            data,
            geometry,
        }
    }

    /// Creates a zero-filled reader of `len` bytes.
    #[must_use]
    pub fn zeroed(len: usize) -> Self {
        Self::new(vec![0u8; len])
    }

    /// Borrows the underlying bytes.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

impl BlockReader for MemoryReader {
    fn id(&self) -> SourceId {
        self.id
    }

    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn geometry(&self) -> &BlockGeometry {
        &self.geometry
    }

    fn describe(&self) -> String {
        format!("memory ({} bytes)", self.data.len())
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.len(), offset, buffer.len())?;
        let start = usize::try_from(offset).map_err(|_| BlockError::IntegerOverflow)?;
        let end = start
            .checked_add(buffer.len())
            .ok_or(BlockError::IntegerOverflow)?;
        let src = self.data.get(start..end).ok_or(BlockError::OutOfBounds {
            offset,
            length: buffer.len() as u64,
            source_len: self.len(),
        })?;
        buffer.copy_from_slice(src);
        Ok(buffer.len())
    }
}
