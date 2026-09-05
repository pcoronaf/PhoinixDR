//! Carving errors.

use phoinix_block::BlockError;
use phoinix_fs::FsError;

/// Errors of the carving engine.
#[derive(Debug, thiserror::Error)]
pub enum CarveError {
    /// Block I/O failed.
    #[error(transparent)]
    Block(#[from] BlockError),
    /// A filesystem engine failed.
    #[error(transparent)]
    Fs(#[from] FsError),
    /// A read went past the end of the readable region.
    #[error("read of {length} bytes at offset {offset} exceeds the region end {limit}")]
    Truncated {
        /// Requested offset.
        offset: u64,
        /// Requested length.
        length: u64,
        /// Exclusive end of the readable region.
        limit: u64,
    },
    /// Signature definitions could not be parsed.
    #[error("invalid signature definition: {0}")]
    Signature(String),
    /// A carved object could not be resolved.
    #[error("{0}")]
    NotFound(String),
    /// Arithmetic overflow while interpreting on-disk values.
    #[error("arithmetic overflow while interpreting on-disk values")]
    Overflow,
}

impl From<CarveError> for FsError {
    fn from(e: CarveError) -> Self {
        match e {
            CarveError::Fs(inner) => inner,
            CarveError::Block(b) => FsError::from(b),
            CarveError::NotFound(m) => FsError::NotFound(m),
            other => FsError::Malformed {
                structure: "carving",
                detail: other.to_string(),
            },
        }
    }
}

impl From<phoinix_core::arith::ArithmeticOverflow> for CarveError {
    fn from(_: phoinix_core::arith::ArithmeticOverflow) -> Self {
        CarveError::Overflow
    }
}
