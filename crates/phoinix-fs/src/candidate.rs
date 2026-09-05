//! Recovery candidates and the provider contract.

use std::fmt;
use std::io::Read;

use phoinix_core::{CandidateId, FileSystemType, SourceId};
use phoinix_health::{RecoveryEvidence, RecoveryHealth};
use serde::{Deserialize, Serialize};

use crate::FsError;
use crate::stream::Extent;

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
    /// An ext2/3/4 inode (and the generation it had when the candidate
    /// was built).
    Ext {
        /// Inode number.
        inode: u32,
        /// Inode generation.
        generation: u32,
    },
    /// A file found by signature carving, identified by the volume byte
    /// offset of its header. No filesystem structure describes it.
    Carved {
        /// Byte offset of the file header inside the volume.
        offset: u64,
        /// Signature identifier (`jpeg`, `pdf`, …).
        type_id: String,
        /// Typical extension of the type, used for the synthetic name.
        extension: String,
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
            FileSystemObjectId::Ext { .. } => FileSystemType::Ext,
            FileSystemObjectId::Carved { .. } => FileSystemType::Unknown,
        }
    }

    /// Whether the object was found by carving rather than through
    /// filesystem metadata.
    #[must_use]
    pub const fn is_carved(&self) -> bool {
        matches!(self, FileSystemObjectId::Carved { .. })
    }

    /// A short, stable, user-typable reference (`64`, `64:stream`,
    /// `c1048576` for carved objects).
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
            FileSystemObjectId::Ext { inode, .. } => inode.to_string(),
            FileSystemObjectId::Carved { offset, .. } => format!("c{offset}"),
        }
    }

    /// Parses a carved reference (`c<offset>` or `c<offset>:<type>`).
    /// Returns the offset and the optional type identifier.
    #[must_use]
    pub fn parse_carved_reference(text: &str) -> Option<(u64, Option<&str>)> {
        let rest = text.trim().strip_prefix('c')?;
        let (offset, type_id) = match rest.split_once(':') {
            Some((o, t)) => (o, Some(t)),
            None => (rest, None),
        };
        let offset: u64 = offset.parse().ok()?;
        Some((offset, type_id.filter(|t| !t.is_empty())))
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
            FileSystemObjectId::Ext { inode, generation } => write!(f, "ext:{inode}-{generation}"),
            FileSystemObjectId::Carved {
                offset, type_id, ..
            } => write!(f, "carved:{offset}:{type_id}"),
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
    /// object identity (`carved-000001048576.jpg` for carved files).
    #[must_use]
    pub fn display_name(&self) -> String {
        if let Some(name) = &self.original_name {
            return name.clone();
        }
        match &self.filesystem_object {
            FileSystemObjectId::Carved {
                offset, extension, ..
            } => format!("carved-{offset:012}.{extension}"),
            other => format!("unnamed-{}", other.short_reference()),
        }
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

    /// The volume byte extents holding the content of `candidate`, in
    /// logical order. Resident or synthetic content has none. Used to
    /// deduplicate carved hits against metadata candidates.
    ///
    /// # Errors
    ///
    /// Returns [`FsError::NotFound`] if the candidate cannot be resolved.
    fn content_extents(&self, candidate: &RecoveryCandidate) -> Result<Vec<Extent>, FsError>;
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
        let o = FileSystemObjectId::Carved {
            offset: 1_048_576,
            type_id: "jpeg".into(),
            extension: "jpg".into(),
        };
        assert_eq!(o.short_reference(), "c1048576");
        assert_eq!(o.to_string(), "carved:1048576:jpeg");
        assert!(o.is_carved());
        assert_eq!(
            FileSystemObjectId::parse_carved_reference("c1048576"),
            Some((1_048_576, None))
        );
        assert_eq!(
            FileSystemObjectId::parse_carved_reference("c1048576:pdf"),
            Some((1_048_576, Some("pdf")))
        );
        assert_eq!(FileSystemObjectId::parse_carved_reference("1048576"), None);
        assert_eq!(FileSystemObjectId::parse_carved_reference("cx"), None);
    }
}
