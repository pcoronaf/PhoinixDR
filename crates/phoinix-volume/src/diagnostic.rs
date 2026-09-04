//! Diagnostics describing what was wrong (or notable) about a partition table.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A finding produced while reading partition structures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum VolumeDiagnostic {
    /// LBA 0 does not end with `55 AA`.
    InvalidMbrSignature,
    /// LBA 0 looks like a filesystem boot sector rather than a partition table.
    FilesystemBootSectorAtLba0,
    /// MBR entries fail basic sanity checks and were ignored.
    ImplausibleMbrEntries,
    /// An MBR entry has status other than `0x00`/`0x80`.
    InvalidMbrEntryStatus {
        /// Entry index (1-based).
        index: u32,
    },
    /// A GPT was found without the protective MBR that should precede it.
    ProtectiveMbrMissing,
    /// A protective MBR (`0xEE`) exists but no valid GPT header was found.
    ProtectiveMbrWithoutGpt,
    /// The primary GPT header failed validation.
    PrimaryGptInvalid {
        /// Why.
        reason: String,
    },
    /// The backup GPT header failed validation.
    BackupGptInvalid {
        /// Why.
        reason: String,
    },
    /// The backup GPT header validated (reported when it was used instead of
    /// the primary).
    BackupGptValid,
    /// The primary header CRC32 does not match.
    GptHeaderCrcMismatch,
    /// The partition entry array CRC32 does not match.
    GptArrayCrcMismatch,
    /// Primary and backup headers disagree on geometry or GUID.
    GptHeadersDisagree,
    /// A partition extends beyond the end of the source.
    PartitionOutsideDevice {
        /// Partition index (1-based).
        index: u32,
    },
    /// Two partitions share bytes.
    OverlappingPartitions {
        /// First partition index (1-based).
        first: u32,
        /// Second partition index (1-based).
        second: u32,
    },
    /// A GPT partition name is not valid UTF-16.
    InvalidUtf16PartitionName {
        /// Partition index (1-based).
        index: u32,
    },
    /// An extended partition chain referenced a sector already visited.
    ExtendedPartitionLoop,
    /// An extended partition chain exceeded the maximum depth.
    ExtendedPartitionTooDeep,
    /// An EBR sector was outside the source or malformed.
    ExtendedPartitionInvalid {
        /// LBA of the offending EBR.
        lba: u64,
        /// Why.
        reason: String,
    },
    /// A partition entry has zero length and was ignored.
    ZeroLengthPartition {
        /// Partition index (1-based).
        index: u32,
    },
}

impl fmt::Display for VolumeDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMbrSignature => write!(f, "LBA 0 lacks the 55 AA boot signature"),
            Self::FilesystemBootSectorAtLba0 => {
                write!(
                    f,
                    "LBA 0 is a filesystem boot sector; the source appears to be a bare volume"
                )
            }
            Self::ImplausibleMbrEntries => {
                write!(f, "MBR partition entries are implausible and were ignored")
            }
            Self::InvalidMbrEntryStatus { index } => {
                write!(f, "MBR entry {index} has an invalid status byte")
            }
            Self::ProtectiveMbrMissing => write!(f, "GPT present without a protective MBR"),
            Self::ProtectiveMbrWithoutGpt => write!(
                f,
                "protective MBR present but no valid GPT header was found"
            ),
            Self::PrimaryGptInvalid { reason } => write!(f, "primary GPT header invalid: {reason}"),
            Self::BackupGptInvalid { reason } => write!(f, "backup GPT header invalid: {reason}"),
            Self::BackupGptValid => write!(f, "backup GPT header is valid and was used"),
            Self::GptHeaderCrcMismatch => write!(f, "GPT header CRC32 mismatch"),
            Self::GptArrayCrcMismatch => write!(f, "GPT partition array CRC32 mismatch"),
            Self::GptHeadersDisagree => write!(f, "primary and backup GPT headers disagree"),
            Self::PartitionOutsideDevice { index } => {
                write!(f, "partition {index} extends beyond the end of the source")
            }
            Self::OverlappingPartitions { first, second } => {
                write!(f, "partitions {first} and {second} overlap")
            }
            Self::InvalidUtf16PartitionName { index } => {
                write!(f, "partition {index} has an invalid UTF-16 name")
            }
            Self::ExtendedPartitionLoop => write!(f, "extended partition chain loops"),
            Self::ExtendedPartitionTooDeep => write!(f, "extended partition chain is too deep"),
            Self::ExtendedPartitionInvalid { lba, reason } => {
                write!(f, "EBR at LBA {lba} is invalid: {reason}")
            }
            Self::ZeroLengthPartition { index } => write!(f, "partition {index} has zero length"),
        }
    }
}
