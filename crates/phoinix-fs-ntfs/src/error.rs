//! NTFS error type.

use phoinix_block::BlockError;
use phoinix_core::{ArithmeticOverflow, RangeError};
use phoinix_fs::FsError;
use thiserror::Error;

/// Errors produced by the NTFS engine.
#[derive(Debug, Error)]
pub enum NtfsError {
    /// A block-layer error.
    #[error(transparent)]
    Block(#[from] BlockError),

    /// Arithmetic on on-disk values overflowed.
    #[error("integer overflow in NTFS structure")]
    Overflow,

    /// The boot sector failed validation.
    #[error("invalid NTFS boot sector: {0}")]
    InvalidBootSector(String),

    /// A FILE record is not usable.
    #[error("invalid FILE record {record}: {reason}")]
    InvalidRecord {
        /// MFT record number.
        record: u64,
        /// What is wrong.
        reason: String,
    },

    /// The update sequence array check failed for one sector of a record.
    #[error("fixup mismatch in record {record} at sector {sector_index}")]
    FixupMismatch {
        /// MFT record number.
        record: u64,
        /// Index of the sector whose tail did not match.
        sector_index: u32,
    },

    /// An attribute is malformed.
    #[error("invalid attribute at offset {offset} in record {record}: {reason}")]
    InvalidAttribute {
        /// MFT record number.
        record: u64,
        /// Byte offset of the attribute inside the record.
        offset: usize,
        /// What is wrong.
        reason: String,
    },

    /// A runlist (mapping pairs array) is malformed.
    #[error("invalid runlist: {0}")]
    InvalidRunlist(String),

    /// The requested record does not exist.
    #[error("MFT record {0} does not exist")]
    NoSuchRecord(u64),

    /// A data stream has no run covering the requested VCN.
    #[error("no extent covers VCN {vcn}; the runlist is incomplete")]
    MissingExtent {
        /// Virtual cluster number without a run.
        vcn: u64,
    },

    /// The record or attribute requires a feature not yet implemented.
    #[error("unsupported NTFS feature: {0}")]
    Unsupported(String),

    /// A required structure was not found.
    #[error("not found: {0}")]
    NotFound(String),
}

impl From<ArithmeticOverflow> for NtfsError {
    fn from(_: ArithmeticOverflow) -> Self {
        NtfsError::Overflow
    }
}

impl From<RangeError> for NtfsError {
    fn from(err: RangeError) -> Self {
        match err {
            RangeError::Overflow { .. } => NtfsError::Overflow,
            other => NtfsError::Block(other.into()),
        }
    }
}

impl From<NtfsError> for FsError {
    fn from(err: NtfsError) -> Self {
        match err {
            NtfsError::Block(b) => FsError::Block(b),
            NtfsError::Overflow => FsError::Overflow,
            NtfsError::InvalidBootSector(d) => FsError::Malformed {
                structure: "NTFS boot sector",
                detail: d,
            },
            NtfsError::InvalidRecord { record, reason } => FsError::Malformed {
                structure: "NTFS FILE record",
                detail: format!("record {record}: {reason}"),
            },
            NtfsError::FixupMismatch {
                record,
                sector_index,
            } => FsError::Malformed {
                structure: "NTFS FILE record",
                detail: format!("record {record}: fixup mismatch at sector {sector_index}"),
            },
            NtfsError::InvalidAttribute {
                record,
                offset,
                reason,
            } => FsError::Malformed {
                structure: "NTFS attribute",
                detail: format!("record {record} offset {offset}: {reason}"),
            },
            NtfsError::InvalidRunlist(d) => FsError::Malformed {
                structure: "NTFS runlist",
                detail: d,
            },
            NtfsError::NoSuchRecord(r) => FsError::NotFound(format!("MFT record {r}")),
            NtfsError::MissingExtent { vcn } => FsError::Malformed {
                structure: "NTFS runlist",
                detail: format!("no extent covers VCN {vcn}"),
            },
            NtfsError::Unsupported(d) => FsError::Unsupported(d),
            NtfsError::NotFound(d) => FsError::NotFound(d),
        }
    }
}
