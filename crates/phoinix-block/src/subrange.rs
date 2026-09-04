//! A bounded window over another reader.

use std::sync::Arc;

use phoinix_core::{ByteRange, SourceId, arith};

use crate::{BlockError, BlockGeometry, BlockReader, check_request};

/// A [`BlockReader`] exposing a contiguous byte range of a parent reader as an
/// independent source. Partitions are `SubrangeReader`s over the disk.
///
/// Every request is validated against the subrange before it is translated
/// and forwarded, so a filesystem parser can never read outside its
/// partition.
#[derive(Clone)]
pub struct SubrangeReader {
    id: SourceId,
    parent: Arc<dyn BlockReader>,
    range: ByteRange,
    geometry: BlockGeometry,
}

impl std::fmt::Debug for SubrangeReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubrangeReader")
            .field("id", &self.id)
            .field("parent", &self.parent.describe())
            .field("range", &self.range)
            .field("geometry", &self.geometry)
            .finish()
    }
}

impl SubrangeReader {
    /// Creates a view of `range` inside `parent`, inheriting its geometry.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::OutOfBounds`] if the range does not fit inside
    /// the parent, or [`BlockError::IntegerOverflow`].
    pub fn new(parent: Arc<dyn BlockReader>, range: ByteRange) -> Result<Self, BlockError> {
        let bounded = ByteRange::bounded(range.offset, range.length, parent.len())?;
        let geometry = parent.geometry().clone();
        Ok(Self {
            id: SourceId::new(),
            parent,
            range: bounded,
            geometry,
        })
    }

    /// Creates a view from a start offset and length.
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_bounds(
        parent: Arc<dyn BlockReader>,
        start: u64,
        length: u64,
    ) -> Result<Self, BlockError> {
        Self::new(parent, ByteRange::new(start, length)?)
    }

    /// The parent reader.
    #[must_use]
    pub fn parent(&self) -> &Arc<dyn BlockReader> {
        &self.parent
    }

    /// The window inside the parent.
    #[must_use]
    pub const fn range(&self) -> ByteRange {
        self.range
    }

    /// Offset of this window inside the parent.
    #[must_use]
    pub const fn start(&self) -> u64 {
        self.range.offset
    }

    /// Translates a subrange-relative offset to a parent offset.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::OutOfBounds`] if `offset` is beyond the window.
    pub fn to_parent_offset(&self, offset: u64) -> Result<u64, BlockError> {
        if offset > self.range.length {
            return Err(BlockError::OutOfBounds {
                offset,
                length: 0,
                source_len: self.range.length,
            });
        }
        Ok(arith::add(self.range.offset, offset)?)
    }
}

impl BlockReader for SubrangeReader {
    fn id(&self) -> SourceId {
        self.id
    }

    fn len(&self) -> u64 {
        self.range.length
    }

    fn geometry(&self) -> &BlockGeometry {
        &self.geometry
    }

    fn describe(&self) -> String {
        format!(
            "{} [{}+{}]",
            self.parent.describe(),
            self.range.offset,
            self.range.length
        )
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.range.length, offset, buffer.len())?;
        let parent_offset = self.to_parent_offset(offset)?;
        self.parent.read_at(parent_offset, buffer)
    }
}
