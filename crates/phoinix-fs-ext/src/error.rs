//! ext errors.

use phoinix_block::BlockError;
use phoinix_fs::FsError;

/// Errors of the ext engine.
#[derive(Debug, thiserror::Error)]
pub enum ExtError {
    /// Block I/O failed.
    #[error(transparent)]
    Block(#[from] BlockError),
    /// The superblock is not an ext superblock or is inconsistent.
    #[error("invalid superblock: {0}")]
    InvalidSuperblock(String),
    /// A structure is malformed.
    #[error("malformed {structure}: {detail}")]
    Malformed {
        /// Which structure.
        structure: &'static str,
        /// What is wrong.
        detail: String,
    },
    /// An inode number is out of range or unreadable.
    #[error("inode {0} is out of range")]
    InodeOutOfRange(u32),
    /// A feature PhoinixDR does not support.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The requested object does not exist.
    #[error("not found: {0}")]
    NotFound(String),
    /// Arithmetic overflow while interpreting on-disk values.
    #[error("integer overflow in filesystem structure")]
    Overflow,
}

impl From<phoinix_core::arith::ArithmeticOverflow> for ExtError {
    fn from(_: phoinix_core::arith::ArithmeticOverflow) -> Self {
        Self::Overflow
    }
}

impl From<ExtError> for FsError {
    fn from(e: ExtError) -> Self {
        match e {
            ExtError::Block(b) => FsError::Block(b),
            ExtError::Overflow => FsError::Overflow,
            ExtError::Unsupported(s) => FsError::Unsupported(s),
            ExtError::NotFound(s) => FsError::NotFound(s),
            ExtError::InodeOutOfRange(n) => FsError::NotFound(format!("inode {n} is out of range")),
            ExtError::InvalidSuperblock(d) => FsError::Malformed {
                structure: "ext superblock",
                detail: d,
            },
            ExtError::Malformed { structure, detail } => FsError::Malformed { structure, detail },
        }
    }
}
