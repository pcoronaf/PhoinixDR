//! Volume-layer error type.

use phoinix_block::BlockError;
use phoinix_core::{ArithmeticOverflow, RangeError};
use thiserror::Error;

/// Errors that prevent partition discovery from producing any result.
///
/// Malformed tables are *not* errors; they surface as
/// [`VolumeDiagnostic`](crate::VolumeDiagnostic)s.
#[derive(Debug, Error)]
pub enum VolumeError {
    /// The source is too small to hold a partition table.
    #[error("source too small for a partition table: {len} bytes")]
    SourceTooSmall {
        /// Source length.
        len: u64,
    },

    /// A block-layer error.
    #[error(transparent)]
    Block(#[from] BlockError),

    /// Arithmetic on sector or byte values overflowed.
    #[error("integer overflow in partition geometry")]
    Overflow,
}

impl From<ArithmeticOverflow> for VolumeError {
    fn from(_: ArithmeticOverflow) -> Self {
        VolumeError::Overflow
    }
}

impl From<RangeError> for VolumeError {
    fn from(err: RangeError) -> Self {
        match err {
            RangeError::Overflow { .. } => VolumeError::Overflow,
            other => VolumeError::Block(other.into()),
        }
    }
}
