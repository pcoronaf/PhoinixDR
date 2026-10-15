//! Errors and refusals.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Why a target is refused. Refusals are decided before anything is
/// written and are part of the public contract: a front-end can show them
/// next to the target before the user chooses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Refusal {
    /// The disk hosts the running operating system. No override exists.
    SystemDisk,
    /// The disk holds the running executable.
    ExecutableDisk,
    /// The disk holds the sessions directory.
    SessionsDisk,
    /// The path is open as a recovery source in this process.
    OpenSource,
    /// A volume on the target is mounted and could not be dismounted.
    Mounted {
        /// The mount point or drive letter.
        mount: String,
    },
    /// The device could not be enumerated or its serial is unknown and no
    /// path-based confirmation was given.
    NotConfirmed {
        /// What the user must type to confirm.
        expected: String,
    },
    /// The partition index does not exist on the device.
    NoSuchPartition {
        /// Requested index.
        index: u32,
    },
    /// The range lies outside the device.
    OutOfBounds,
    /// The platform cannot write to devices.
    Unsupported {
        /// Detail.
        reason: String,
    },
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SystemDisk => f.write_str("this disk hosts the running operating system"),
            Self::ExecutableDisk => f.write_str("this disk holds the running PhoinixDR executable"),
            Self::SessionsDisk => f.write_str("this disk holds the PhoinixDR sessions directory"),
            Self::OpenSource => f.write_str("this source is open for recovery in this process"),
            Self::Mounted { mount } => write!(f, "a volume on the target is mounted at {mount}"),
            Self::NotConfirmed { expected } => {
                write!(f, "the confirmation text does not match; type {expected:?}")
            }
            Self::NoSuchPartition { index } => write!(f, "the device has no partition {index}"),
            Self::OutOfBounds => f.write_str("the range lies outside the device"),
            Self::Unsupported { reason } => write!(f, "not supported: {reason}"),
        }
    }
}

/// Errors of an erase.
#[derive(Debug, thiserror::Error)]
pub enum EraseError {
    /// The target was refused before anything was written.
    #[error("refused: {0}")]
    Refused(Refusal),
    /// The device layer failed (enumeration, read-back).
    #[error(transparent)]
    Device(#[from] phoinix_device::DeviceError),
    /// An I/O error that could not be worked around.
    #[error("I/O error on {path}: {source}")]
    Io {
        /// The path involved.
        path: PathBuf,
        /// The error.
        #[source]
        source: std::io::Error,
    },
    /// Invalid request.
    #[error("invalid request: {0}")]
    Invalid(String),
    /// The erase was cancelled; the target is partially erased.
    #[error("erase cancelled")]
    Cancelled,
}

impl EraseError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
