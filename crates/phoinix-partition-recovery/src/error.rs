//! Errors of the partition search.

use phoinix_block::BlockError;
use phoinix_carve::CarveError;

/// Errors of the partition search.
#[derive(Debug, thiserror::Error)]
pub enum PartitionRecoveryError {
    /// Block I/O failed.
    #[error(transparent)]
    Block(#[from] BlockError),
    /// The header search failed.
    #[error(transparent)]
    Scan(#[from] CarveError),
    /// Arithmetic on on-disk values overflowed.
    #[error("integer overflow in filesystem structure")]
    Overflow,
}

impl From<phoinix_core::arith::ArithmeticOverflow> for PartitionRecoveryError {
    fn from(_: phoinix_core::arith::ArithmeticOverflow) -> Self {
        Self::Overflow
    }
}
