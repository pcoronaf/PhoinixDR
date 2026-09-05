//! RAW images split into numbered segment files.

use std::path::{Path, PathBuf};

use phoinix_block::{BlockError, BlockGeometry, BlockReader, RawImage, check_request};
use phoinix_core::SourceId;

use crate::ImageError;

/// A [`BlockReader`] over the concatenation of segment files.
#[derive(Debug)]
pub struct SplitRawImage {
    id: SourceId,
    first: PathBuf,
    segments: Vec<(u64, RawImage)>,
    length: u64,
    geometry: BlockGeometry,
}

impl SplitRawImage {
    /// Opens the given segment files, in order.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError`] if a file cannot be opened or the total
    /// length overflows.
    pub fn open(paths: &[PathBuf]) -> Result<Self, ImageError> {
        let first = paths
            .first()
            .cloned()
            .ok_or_else(|| ImageError::Unsupported("no segment files".into()))?;
        let mut segments = Vec::with_capacity(paths.len());
        let mut length = 0u64;
        for p in paths {
            let raw = RawImage::open(p)?;
            let len = raw.len();
            segments.push((length, raw));
            length = length.checked_add(len).ok_or(ImageError::Overflow)?;
        }
        Ok(Self {
            id: SourceId::new(),
            first,
            segments,
            length,
            geometry: BlockGeometry::SECTOR_512,
        })
    }

    /// The segment paths.
    #[must_use]
    pub fn paths(&self) -> Vec<PathBuf> {
        self.segments
            .iter()
            .map(|(_, r)| r.path().to_path_buf())
            .collect()
    }

    /// The first segment's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.first
    }
}

impl BlockReader for SplitRawImage {
    fn id(&self) -> SourceId {
        self.id
    }

    fn len(&self) -> u64 {
        self.length
    }

    fn geometry(&self) -> &BlockGeometry {
        &self.geometry
    }

    fn describe(&self) -> String {
        format!(
            "{} (+{} segments)",
            self.first.display(),
            self.segments.len().saturating_sub(1)
        )
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.length, offset, buffer.len())?;
        if buffer.is_empty() {
            return Ok(0);
        }
        // Locate the segment holding `offset`.
        let idx = self
            .segments
            .partition_point(|(start, _)| *start <= offset)
            .saturating_sub(1);
        let (start, raw) = self.segments.get(idx).ok_or(BlockError::OutOfBounds {
            offset,
            length: buffer.len() as u64,
            source_len: self.length,
        })?;
        let within = offset - start;
        let available = raw.len().saturating_sub(within);
        if available == 0 {
            // An empty segment in the middle: hand the request on.
            return Ok(0);
        }
        let want = usize::try_from(available.min(buffer.len() as u64))
            .map_err(|_| BlockError::IntegerOverflow)?;
        let dst = buffer.get_mut(..want).ok_or(BlockError::IntegerOverflow)?;
        raw.read_at(within, dst)
    }
}
