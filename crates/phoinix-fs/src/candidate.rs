//! Recovery candidates and the provider contract.

use std::fmt;
use std::io::Read;

use phoinix_core::{CandidateId, FileSystemType, SourceId};
use phoinix_health::{RecoveryEvidence, RecoveryHealth};
use serde::{Deserialize, Serialize};

use crate::FsError;

/// Filesystem-specific identity of a candidate's object.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "filesystem", rename_all = "kebab-case")]
pub enum FileSystemObjectId {
    /// An NTFS MFT record (and optionally a named data stream).
    Ntfs {
        /// MFT record number.
        record: u64,
        /// Sequence number of the record when the candidate was built.
        sequence: u16,
        /// Named stream, or `None` for the unnamed stream.
        stream: Option<String>,
    },
    /// A FAT12/16/32 directory entry, identified by the volume byte offset
    /// of its 8.3 entry.
    Fat {
        /// Byte offset of the short-name directory entry inside the volume.
        entry_offset: u64,
    },
    /// An exFAT directory entry set, identified by the volume byte offset of
    /// its File entry.
    ExFat {
        /// Byte offset of the File directory entry inside the volume.
        entry_offset: u64,
    },
}

impl FileSystemObjectId {
    /// The filesystem the object belongs to.
    #[must_use]
    pub const fn filesystem(&self) -> FileSystemType {
        match self {
            FileSystemObjectId::Ntfs { .. } => FileSystemType::Ntfs,
            FileSystemObjectId::Fat { .. } => FileSystemType::Fat32,
            FileSystemObjectId::ExFat { .. } => FileSystemType::ExFat,
        }
    }

    /// A short, stable, user-typable reference (`64`, `64:stream`).
    #[must_use]
    pub fn short_reference(&self) -> String {
        match self {
            FileSystemObjectId::Ntfs {
                record,
                stream: None,
                ..
            } => record.to_string(),
            FileSystemObjectId::Ntfs {
                record,
                stream: Some(s),
                ..
            } => format!("{record}:{s}"),
            FileSystemObjectId::Fat { entry_offset }
            | FileSystemObjectId::ExFat { entry_offset } => entry_offset.to_string(),
        }
    }
}

impl fmt::Display for FileSystemObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileSystemObjectId::Ntfs {
                record,
                sequence,
                stream,
            } => {
                write!(f, "ntfs:{record}-{sequence}")?;
                if let Some(s) = stream {
                    write!(f, ":{s}")?;
                }
                Ok(())
            }
            FileSystemObjectId::Fat { entry_offset } => write!(f, "fat:{entry_offset}"),
            FileSystemObjectId::ExFat { entry_offset } => write!(f, "exfat:{entry_offset}"),
        }
    }
}

/// Timestamps of a candidate, as Unix seconds and preformatted ISO-8601.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CandidateTimestamps {
    /// Creation time (Unix seconds).
    pub created: Option<i64>,
    /// Modification time (Unix seconds).
    pub modified: Option<i64>,
    /// Access time (Unix seconds).
    pub accessed: Option<i64>,
    /// Creation time, ISO-8601 UTC.
    pub created_iso: Option<String>,
    /// Modification time, ISO-8601 UTC.
    pub modified_iso: Option<String>,
    /// Access time, ISO-8601 UTC.
    pub accessed_iso: Option<String>,
}

/// A potentially recoverable object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryCandidate {
    /// Unique identifier of this candidate within a scan.
    pub id: CandidateId,
    /// Source the candidate was found on.
    pub source_id: SourceId,
    /// Filesystem type.
    pub filesystem: FileSystemType,
    /// Filesystem-specific object identity.
    pub filesystem_object: FileSystemObjectId,
    /// Original filename, if recovered.
    pub original_name: Option<String>,
    /// Original path, if reconstructed (prefixed `\?\` when uncertain).
    pub original_path: Option<String>,
    /// Whether the reconstructed path is uncertain.
    pub path_uncertain: bool,
    /// Logical size in bytes, if known.
    pub logical_size: Option<u64>,
    /// Whether the object is deleted (as opposed to an allocated file).
    pub deleted: bool,
    /// Timestamps.
    pub timestamps: CandidateTimestamps,
    /// The evidence gathered.
    pub evidence: RecoveryEvidence,
    /// The assessment derived from the evidence.
    pub health: RecoveryHealth,
}

impl RecoveryCandidate {
    /// A display name: the original name, or a synthetic one from the
    /// object identity.
    #[must_use]
    pub fn display_name(&self) -> String {
        self.original_name
            .clone()
            .unwrap_or_else(|| format!("unnamed-{}", self.filesystem_object.short_reference()))
    }
}

/// Readable content of a candidate.
pub trait CandidateContent: Read + Send {
    /// Expected length in bytes.
    fn len(&self) -> u64;

    /// Whether the content is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A filesystem engine that can enumerate deleted files and hand out their
/// content. Generic code (CLI, recovery writer) depends on this contract
/// only.
pub trait DeletedFileProvider: Send + Sync {
    /// Enumerates deleted candidates. Damaged records that yield no candidate
    /// are skipped; errors are reported per item so enumeration continues.
    fn deleted_files(&self) -> Box<dyn Iterator<Item = Result<RecoveryCandidate, FsError>> + '_>;

    /// Rebuilds the candidate for `object` (used to address candidates
    /// without a session database, see ADR-0008).
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NotFound`] if the object does not exist or is not
    /// a candidate.
    fn candidate(&self, object: &FileSystemObjectId) -> Result<RecoveryCandidate, FsError>;

    /// Parses a short reference as printed by `scan` (the inverse of
    /// [`FileSystemObjectId::short_reference`]) for this engine.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NotFound`] if the text is not a valid reference.
    fn object_from_reference(&self, text: &str) -> Result<FileSystemObjectId, FsError>;

    /// Opens the content of `candidate` for streaming.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::Unsupported`] for content PHOINIX cannot decode
    /// (compressed, encrypted) or [`FsError::NotFound`].
    fn open_content(
        &self,
        candidate: &RecoveryCandidate,
    ) -> Result<Box<dyn CandidateContent>, FsError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_references() {
        let o = FileSystemObjectId::Ntfs {
            record: 64,
            sequence: 3,
            stream: None,
        };
        assert_eq!(o.short_reference(), "64");
        assert_eq!(o.to_string(), "ntfs:64-3");
        let o = FileSystemObjectId::Ntfs {
            record: 64,
            sequence: 3,
            stream: Some("secret".into()),
        };
        assert_eq!(o.short_reference(), "64:secret");
        assert_eq!(o.to_string(), "ntfs:64-3:secret");
        assert_eq!(o.filesystem(), FileSystemType::Ntfs);
    }
}
