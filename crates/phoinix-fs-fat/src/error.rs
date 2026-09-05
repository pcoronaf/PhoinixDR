//! FAT error type.

use phoinix_block::BlockError;
use phoinix_core::{ArithmeticOverflow, RangeError};
use phoinix_fs::FsError;
use thiserror::Error;

/// Errors produced by the FAT engine.
#[derive(Debug, Error)]
pub enum FatError {
    /// A block-layer error.
    #[error(transparent)]
    Block(#[from] BlockError),
    /// Arithmetic on on-disk values overflowed.
    #[error("integer overflow in FAT structure")]
    Overflow,
    /// The boot sector failed validation.
    #[error("invalid FAT boot sector: {0}")]
    InvalidBootSector(String),
    /// A cluster chain is malformed.
    #[error("invalid cluster chain: {0}")]
    InvalidChain(String),
    /// The requested object does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// A feature PhoinixDR does not support.
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl From<ArithmeticOverflow> for FatError {
    fn from(_: ArithmeticOverflow) -> Self {
        FatError::Overflow
    }
}

impl From<RangeError> for FatError {
    fn from(err: RangeError) -> Self {
        match err {
            RangeError::Overflow { .. } => FatError::Overflow,
            other => FatError::Block(other.into()),
        }
    }
}

impl From<FatError> for FsError {
    fn from(err: FatError) -> Self {
        match err {
            FatError::Block(b) => FsError::Block(b),
            FatError::Overflow => FsError::Overflow,
            FatError::InvalidBootSector(d) => FsError::Malformed {
                structure: "FAT boot sector",
                detail: d,
            },
            FatError::InvalidChain(d) => FsError::Malformed {
                structure: "FAT cluster chain",
                detail: d,
            },
            FatError::NotFound(d) => FsError::NotFound(d),
            FatError::Unsupported(d) => FsError::Unsupported(d),
        }
    }
}

impl From<FsError> for FatError {
    fn from(err: FsError) -> Self {
        match err {
            FsError::Block(b) => FatError::Block(b),
            FsError::Overflow => FatError::Overflow,
            FsError::Malformed { detail, .. } => FatError::InvalidChain(detail),
            FsError::Unsupported(d) => FatError::Unsupported(d),
            FsError::NotFound(d) => FatError::NotFound(d),
        }
    }
}
