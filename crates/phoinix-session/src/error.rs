//! Service-layer errors: one type that every front-end can display.

use phoinix_carve::CarveError;
use phoinix_device::DeviceError;
use phoinix_fs::FsError;
use phoinix_partition_recovery::PartitionRecoveryError;
use phoinix_recovery::RecoveryError;
use phoinix_volume::VolumeError;

/// Errors of the service layer.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Device access failed.
    #[error(transparent)]
    Device(#[from] DeviceError),
    /// Block I/O failed.
    #[error(transparent)]
    Block(#[from] phoinix_block::BlockError),
    /// Partition table could not be read.
    #[error(transparent)]
    Volume(#[from] VolumeError),
    /// A filesystem engine failed.
    #[error(transparent)]
    Fs(#[from] FsError),
    /// Carving failed.
    #[error(transparent)]
    Carve(#[from] CarveError),
    /// The structure search failed.
    #[error(transparent)]
    Partitions(#[from] PartitionRecoveryError),
    /// Recovery failed.
    #[error(transparent)]
    Recovery(#[from] RecoveryError),
    /// I/O on session files or destinations failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A session file could not be parsed.
    #[error("invalid session file: {0}")]
    InvalidSession(String),
    /// The requested object does not exist.
    #[error("{0}")]
    NotFound(String),
    /// The requested operation is not possible in this state.
    #[error("{0}")]
    Invalid(String),
    /// The scan was cancelled.
    #[error("the scan was cancelled")]
    Cancelled,
}

impl SessionError {
    /// Wraps an I/O error with context.
    #[must_use]
    pub fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

impl From<SessionError> for String {
    fn from(e: SessionError) -> Self {
        e.to_string()
    }
}
