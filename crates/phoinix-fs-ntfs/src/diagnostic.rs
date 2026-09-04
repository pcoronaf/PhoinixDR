//! Diagnostics attached to files, paths and candidates.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A finding made while interpreting NTFS structures. Diagnostics never abort
/// processing; they qualify the result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum NtfsDiagnostic {
    /// Attribute parsing stopped early; later attributes were lost.
    AttributeError {
        /// Offset of the offending attribute.
        offset: usize,
        /// Why.
        reason: String,
    },
    /// An attribute type PHOINIX does not interpret was skipped.
    UnknownAttribute {
        /// Type code.
        code: u32,
    },
    /// The `$ATTRIBUTE_LIST` could not be fully honoured.
    AttributeListIncomplete {
        /// Why.
        reason: String,
    },
    /// An extension record named by the attribute list could not be used.
    ExtensionRecordUnavailable {
        /// Record number.
        record: u64,
        /// Why.
        reason: String,
    },
    /// A data stream is NTFS-compressed; content recovery is unsupported.
    CompressedStream {
        /// Stream name.
        name: Option<String>,
    },
    /// A data stream is EFS-encrypted; content is unusable without keys.
    EncryptedStream {
        /// Stream name.
        name: Option<String>,
    },
    /// A runlist could not be decoded completely.
    RunlistError {
        /// Stream name.
        name: Option<String>,
        /// Why.
        reason: String,
    },
    /// The runlist does not cover the whole allocated size.
    RunlistIncomplete {
        /// Stream name.
        name: Option<String>,
        /// Clusters covered by runs.
        covered_clusters: u64,
        /// Clusters expected from the allocated size.
        expected_clusters: u64,
    },
    /// The parent reference points at a record that has since been reused.
    ParentReferenceStale {
        /// Parent record number.
        parent: u64,
        /// Sequence number stored in the reference.
        expected_sequence: u16,
        /// Sequence number of the record now.
        actual_sequence: u16,
    },
    /// The parent directory record is itself deleted (its name still exists).
    ParentDeleted {
        /// Parent record number.
        parent: u64,
    },
    /// The parent record could not be read.
    ParentUnreadable {
        /// Parent record number.
        parent: u64,
        /// Why.
        reason: String,
    },
    /// The parent record has no usable name.
    ParentUnnamed {
        /// Parent record number.
        parent: u64,
    },
    /// The parent chain exceeded the depth limit.
    PathDepthExceeded,
    /// The parent chain loops.
    PathLoop,
    /// The record has no `$FILE_NAME`; the file is anonymous.
    NoFileName,
    /// A name was not valid UTF-16 and was decoded lossily.
    NameInvalidUtf16,
}

impl fmt::Display for NtfsDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stream = |name: &Option<String>| {
            name.as_deref()
                .map_or_else(|| "unnamed stream".to_owned(), |n| format!("stream {n:?}"))
        };
        match self {
            Self::AttributeError { offset, reason } => {
                write!(f, "attribute parsing stopped at offset {offset}: {reason}")
            }
            Self::UnknownAttribute { code } => {
                write!(f, "unknown attribute type {code:#x} skipped")
            }
            Self::AttributeListIncomplete { reason } => {
                write!(f, "$ATTRIBUTE_LIST incomplete: {reason}")
            }
            Self::ExtensionRecordUnavailable { record, reason } => {
                write!(f, "extension record {record} unavailable: {reason}")
            }
            Self::CompressedStream { name } => write!(
                f,
                "{} is NTFS-compressed (recovery unsupported)",
                stream(name)
            ),
            Self::EncryptedStream { name } => write!(f, "{} is EFS-encrypted", stream(name)),
            Self::RunlistError { name, reason } => {
                write!(f, "{} runlist invalid: {reason}", stream(name))
            }
            Self::RunlistIncomplete {
                name,
                covered_clusters,
                expected_clusters,
            } => {
                write!(
                    f,
                    "{} runlist covers {covered_clusters} of {expected_clusters} clusters",
                    stream(name)
                )
            }
            Self::ParentReferenceStale {
                parent,
                expected_sequence,
                actual_sequence,
            } => write!(
                f,
                "parent record {parent} has been reused (sequence {actual_sequence}, expected {expected_sequence})"
            ),
            Self::ParentDeleted { parent } => {
                write!(f, "parent directory record {parent} is deleted")
            }
            Self::ParentUnreadable { parent, reason } => {
                write!(f, "parent record {parent} unreadable: {reason}")
            }
            Self::ParentUnnamed { parent } => write!(f, "parent record {parent} has no name"),
            Self::PathDepthExceeded => write!(f, "parent chain exceeds the depth limit"),
            Self::PathLoop => write!(f, "parent chain loops"),
            Self::NoFileName => write!(f, "record has no $FILE_NAME attribute"),
            Self::NameInvalidUtf16 => write!(f, "name contained invalid UTF-16"),
        }
    }
}
