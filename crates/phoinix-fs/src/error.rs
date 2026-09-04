//! Filesystem-layer error type.

use phoinix_block::BlockError;
use phoinix_core::{ArithmeticOverflow, RangeError};
use thiserror::Error;

/// Errors shared by filesystem engines.
#[derive(Debug, Error)]
pub enum FsError {
    /// A block-layer error.
    #[error(transparent)]
    Block(#[from] BlockError),

    /// Arithmetic on on-disk values overflowed.
    #[error("integer overflow in filesystem structure")]
    Overflow,

    /// The structure is malformed.
    #[error("malformed {structure}: {detail}")]
    Malformed {
        /// Which structure.
        structure: &'static str,
        /// What is wrong.
        detail: String,
    },

    /// A feature PHOINIX does not support yet.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// The requested object does not exist.
    #[error("not found: {0}")]
    NotFound(String),
}

impl From<ArithmeticOverflow> for FsError {
    fn from(_: ArithmeticOverflow) -> Self {
        FsError::Overflow
    }
}

impl From<RangeError> for FsError {
    fn from(err: RangeError) -> Self {
        match err {
            RangeError::Overflow { .. } => FsError::Overflow,
            other => FsError::Block(other.into()),
        }
    }
}
