//! Device-layer error type.

use phoinix_block::BlockError;
use thiserror::Error;

/// Errors produced while enumerating or opening devices.
#[derive(Debug, Error)]
pub enum DeviceError {
    /// The device does not exist.
    #[error("device not found: {0}")]
    NotFound(String),

    /// Elevated privileges are required.
    #[error("permission denied opening {0}; elevated privileges are required to read raw devices")]
    PermissionDenied(String),

    /// The platform does not support this operation.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// The platform device registry returned malformed data.
    #[error("malformed device metadata for {device}: {detail}")]
    Malformed {
        /// Device name.
        device: String,
        /// What was wrong.
        detail: String,
    },

    /// A block-layer error.
    #[error(transparent)]
    Block(#[from] BlockError),

    /// An operating-system error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
