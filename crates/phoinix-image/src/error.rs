//! Image-container error type.

use std::path::PathBuf;

use phoinix_block::BlockError;
use phoinix_core::ArithmeticOverflow;
use thiserror::Error;

/// Errors produced while opening or reading an image container.
#[derive(Debug, Error)]
pub enum ImageError {
    /// A block-layer error from a segment file.
    #[error(transparent)]
    Block(#[from] BlockError),

    /// A container structure is not what the format requires.
    #[error("malformed {format} image: {detail}")]
    Malformed {
        /// Container format.
        format: &'static str,
        /// What was wrong.
        detail: String,
    },

    /// The container uses a feature PhoinixDR does not read.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// A segment or extent file named by the container is missing.
    #[error("missing segment file {0}")]
    MissingSegment(PathBuf),

    /// Arithmetic on offsets or lengths overflowed.
    #[error("integer overflow")]
    Overflow,
}

impl From<ArithmeticOverflow> for ImageError {
    fn from(_: ArithmeticOverflow) -> Self {
        Self::Overflow
    }
}

impl From<std::io::Error> for ImageError {
    fn from(e: std::io::Error) -> Self {
        Self::Block(BlockError::from(e))
    }
}

/// A block error describing corrupt container data at `what`.
pub(crate) fn corrupt(what: impl Into<String>) -> BlockError {
    BlockError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        what.into(),
    ))
}
