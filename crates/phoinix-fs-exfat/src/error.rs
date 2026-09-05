//! exFAT error type.

use phoinix_block::BlockError;
use phoinix_core::{ArithmeticOverflow, RangeError};
use phoinix_fs::FsError;
use thiserror::Error;

/// Errors produced by the exFAT engine.
#[derive(Debug, Error)]
pub enum ExfatError {
    /// A block-layer error.
    #[error(transparent)]
    Block(#[from] BlockError),
    /// Arithmetic overflow.
    #[error("integer overflow in exFAT structure")]
    Overflow,
    /// The boot sector failed validation.
    #[error("invalid exFAT boot sector: {0}")]
    InvalidBootSector(String),
    /// A directory or chain structure is malformed.
    #[error("malformed exFAT structure: {0}")]
    Malformed(String),
    /// The requested object does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// Unsupported feature.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl From<ArithmeticOverflow> for ExfatError {
    fn from(_: ArithmeticOverflow) -> Self {
        ExfatError::Overflow
    }
}

impl From<RangeError> for ExfatError {
    fn from(err: RangeError) -> Self {
        match err {
            RangeError::Overflow { .. } => ExfatError::Overflow,
            other => ExfatError::Block(other.into()),
        }
    }
}

impl From<ExfatError> for FsError {
    fn from(err: ExfatError) -> Self {
        match err {
            ExfatError::Block(b) => FsError::Block(b),
            ExfatError::Overflow => FsError::Overflow,
            ExfatError::InvalidBootSector(d) => FsError::Malformed {
                structure: "exFAT boot sector",
                detail: d,
            },
            ExfatError::Malformed(d) => FsError::Malformed {
                structure: "exFAT directory",
                detail: d,
            },
            ExfatError::NotFound(d) => FsError::NotFound(d),
            ExfatError::Unsupported(d) => FsError::Unsupported(d),
        }
    }
}

impl From<FsError> for ExfatError {
    fn from(err: FsError) -> Self {
        match err {
            FsError::Block(b) => ExfatError::Block(b),
            FsError::Overflow => ExfatError::Overflow,
            FsError::Malformed { detail, .. } => ExfatError::Malformed(detail),
            FsError::Unsupported(d) => ExfatError::Unsupported(d),
            FsError::NotFound(d) => ExfatError::NotFound(d),
        }
    }
}
