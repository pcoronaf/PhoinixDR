//! The read-only block reader contract and its extension helpers.

use std::sync::Arc;

use phoinix_core::{ByteRange, SourceId, arith};

use crate::{BlockError, BlockGeometry};

/// Maximum number of bytes a single [`BlockReader::read_at`] call may request.
///
/// This bounds allocations driven by malformed metadata. Larger transfers must
/// be split (see [`BlockReaderExt::read_exact_at`]).
pub const MAX_SINGLE_READ: usize = 16 * 1024 * 1024;

/// Read-only random access to a storage source.
///
/// See the crate documentation for the contract of [`read_at`](Self::read_at).
pub trait BlockReader: Send + Sync {
    /// Identifier of this source.
    fn id(&self) -> SourceId;

    /// Total length of the source in bytes.
    fn len(&self) -> u64;

    /// Whether the source has zero length.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Sector geometry of the source.
    fn geometry(&self) -> &BlockGeometry;

    /// Human-readable description (path, device name) for diagnostics.
    fn describe(&self) -> String {
        format!("source {}", self.id())
    }

    /// Reads bytes starting at `offset` into `buffer`, returning the number of
    /// bytes read.
    ///
    /// # Errors
    ///
    /// See the crate-level contract.
    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError>;
}

/// Validates a read request against the reader contract.
///
/// # Errors
///
/// Returns [`BlockError::RequestTooLarge`] or [`BlockError::OutOfBounds`].
pub fn check_request(source_len: u64, offset: u64, length: usize) -> Result<(), BlockError> {
    if length > MAX_SINGLE_READ {
        return Err(BlockError::RequestTooLarge {
            length,
            max: MAX_SINGLE_READ,
        });
    }
    let len_u64 = arith::from_usize(length)?;
    let end = arith::add(offset, len_u64).map_err(|_| BlockError::OutOfBounds {
        offset,
        length: len_u64,
        source_len,
    })?;
    if end > source_len {
        return Err(BlockError::OutOfBounds {
            offset,
            length: len_u64,
            source_len,
        });
    }
    Ok(())
}

/// Convenience helpers implemented for every [`BlockReader`].
pub trait BlockReaderExt: BlockReader {
    /// Fills `buffer` completely from `offset`, splitting the request into
    /// chunks of at most [`MAX_SINGLE_READ`] and retrying short reads.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::ShortRead`] if the source stops delivering bytes
    /// before the buffer is full, or any error from `read_at`.
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<(), BlockError> {
        let total = buffer.len();
        // Validate the whole request up front so a huge request fails before
        // any I/O happens.
        let total_u64 = arith::from_usize(total)?;
        let end = arith::add(offset, total_u64).map_err(|_| BlockError::OutOfBounds {
            offset,
            length: total_u64,
            source_len: self.len(),
        })?;
        if end > self.len() {
            return Err(BlockError::OutOfBounds {
                offset,
                length: total_u64,
                source_len: self.len(),
            });
        }

        let mut done = 0usize;
        while done < total {
            let chunk_len = (total - done).min(MAX_SINGLE_READ);
            let chunk_offset = arith::add(offset, arith::from_usize(done)?)?;
            let chunk = buffer
                .get_mut(done..done + chunk_len)
                .ok_or(BlockError::IntegerOverflow)?;
            let n = self.read_at(chunk_offset, chunk)?;
            if n == 0 {
                return Err(BlockError::ShortRead {
                    expected: total,
                    actual: done,
                });
            }
            done = done.checked_add(n).ok_or(BlockError::IntegerOverflow)?;
        }
        Ok(())
    }

    /// Reads exactly `length` bytes from `offset` into a new vector.
    ///
    /// The length is capped at [`MAX_SINGLE_READ`] so that a malformed
    /// on-disk size cannot trigger an enormous allocation; stream larger
    /// content instead.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::RequestTooLarge`], or any error from
    /// [`read_exact_at`](Self::read_exact_at).
    fn read_vec(&self, offset: u64, length: usize) -> Result<Vec<u8>, BlockError> {
        check_request(self.len(), offset, length)?;
        let mut buffer = vec![0u8; length];
        self.read_exact_at(offset, &mut buffer)?;
        Ok(buffer)
    }

    /// Reads exactly the bytes covered by `range`.
    ///
    /// # Errors
    ///
    /// As [`read_vec`](Self::read_vec).
    fn read_range(&self, range: ByteRange) -> Result<Vec<u8>, BlockError> {
        let length = arith::to_usize(range.length)?;
        self.read_vec(range.offset, length)
    }

    /// Reads one logical sector.
    ///
    /// # Errors
    ///
    /// As [`read_vec`](Self::read_vec).
    fn read_sector(&self, lba: u64) -> Result<Vec<u8>, BlockError> {
        self.read_sectors(lba, 1)
    }

    /// Reads `count` consecutive logical sectors starting at `lba`.
    ///
    /// # Errors
    ///
    /// As [`read_vec`](Self::read_vec).
    fn read_sectors(&self, lba: u64, count: u32) -> Result<Vec<u8>, BlockError> {
        let geometry = self.geometry();
        let offset = geometry.lba_to_offset(lba)?;
        let length = arith::to_usize(arith::mul(
            u64::from(count),
            u64::from(geometry.logical_sector_size),
        )?)?;
        self.read_vec(offset, length)
    }
}

impl<T: BlockReader + ?Sized> BlockReaderExt for T {}

macro_rules! delegate_block_reader {
    ($($ty:ty),* $(,)?) => {
        $(
            impl<T: BlockReader + ?Sized> BlockReader for $ty {
                fn id(&self) -> SourceId {
                    (**self).id()
                }
                fn len(&self) -> u64 {
                    (**self).len()
                }
                fn is_empty(&self) -> bool {
                    (**self).is_empty()
                }
                fn geometry(&self) -> &BlockGeometry {
                    (**self).geometry()
                }
                fn describe(&self) -> String {
                    (**self).describe()
                }
                fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
                    (**self).read_at(offset, buffer)
                }
            }
        )*
    };
}

delegate_block_reader!(Arc<T>, Box<T>, &T);
