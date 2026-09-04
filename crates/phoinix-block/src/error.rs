//! Block-layer error type.

use phoinix_core::{ArithmeticOverflow, RangeError};
use thiserror::Error;

/// Errors produced by block readers.
#[derive(Debug, Error)]
pub enum BlockError {
    /// The requested range is not inside the source.
    #[error("read out of bounds: offset={offset}, length={length}, source length={source_len}")]
    OutOfBounds {
        /// Requested offset.
        offset: u64,
        /// Requested length.
        length: u64,
        /// Length of the source.
        source_len: u64,
    },

    /// Fewer bytes than required were available.
    #[error("short read: expected {expected}, received {actual}")]
    ShortRead {
        /// Bytes expected.
        expected: usize,
        /// Bytes actually read.
        actual: usize,
    },

    /// A single request exceeded [`crate::MAX_SINGLE_READ`].
    #[error("read request too large: {length} bytes exceeds the {max}-byte limit")]
    RequestTooLarge {
        /// Requested length.
        length: usize,
        /// Maximum permitted length.
        max: usize,
    },

    /// The source disappeared or could not be found.
    #[error("source unavailable")]
    SourceUnavailable,

    /// The process lacks permission to read the source.
    #[error("permission denied")]
    PermissionDenied,

    /// The sector geometry is not acceptable.
    #[error("invalid geometry: {0}")]
    InvalidGeometry(String),

    /// An operating-system I/O error.
    #[error("I/O error: {0}")]
    Io(#[source] std::io::Error),

    /// Arithmetic on offsets or lengths overflowed.
    #[error("integer overflow")]
    IntegerOverflow,
}

impl From<std::io::Error> for BlockError {
    fn from(err: std::io::Error) -> Self {
        use std::io::ErrorKind;
        match err.kind() {
            ErrorKind::PermissionDenied => BlockError::PermissionDenied,
            ErrorKind::NotFound => BlockError::SourceUnavailable,
            _ => BlockError::Io(err),
        }
    }
}

impl From<ArithmeticOverflow> for BlockError {
    fn from(_: ArithmeticOverflow) -> Self {
        BlockError::IntegerOverflow
    }
}

impl From<RangeError> for BlockError {
    fn from(err: RangeError) -> Self {
        match err {
            RangeError::Overflow { .. } => BlockError::IntegerOverflow,
            RangeError::OutOfBounds {
                offset,
                length,
                bound,
            } => BlockError::OutOfBounds {
                offset,
                length,
                source_len: bound,
            },
        }
    }
}
