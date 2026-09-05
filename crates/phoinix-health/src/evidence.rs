//! The evidence model.

use serde::{Deserialize, Serialize};

use crate::validate::{FileTypeDetection, ValidationResult};

/// Evidence about the metadata record describing a candidate.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MetadataEvidence {
    /// The record parsed and passed its integrity checks (fixups, headers).
    pub valid_record: bool,
    /// An original filename was recovered.
    pub filename_available: bool,
    /// The original parent directory could be identified.
    pub original_parent_available: bool,
    /// The parent reference still points at the original directory record.
    pub parent_reference_valid: bool,
    /// The logical size is known.
    pub logical_size_available: bool,
    /// The logical size itself, when known (zero means an empty file whose
    /// content is trivially recoverable).
    pub logical_size: Option<u64>,
    /// Timestamps were recovered.
    pub timestamps_available: bool,
}

/// Evidence about where the content lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtentEvidence {
    /// Content is stored inside the metadata record; no clusters involved.
    pub resident: bool,
    /// Every extent of the stream is known.
    pub complete: bool,
    /// Number of physical extents (fragments).
    pub extent_count: u32,
    /// Clusters referenced by data extents, if known.
    pub total_clusters: Option<u64>,
    /// Clusters the stream should occupy according to its allocated size,
    /// if known; compared with `total_clusters` when the map is incomplete.
    pub expected_clusters: Option<u64>,
    /// The stream contains sparse regions.
    pub sparse: bool,
    /// The stream is compressed (content unsupported).
    pub compressed: bool,
    /// The stream is encrypted (content unusable without keys).
    pub encrypted: bool,
    /// The extents come from an intact cluster chain or runlist rather than
    /// being reconstructed. `false` means the layout was inferred (FAT
    /// contiguity assumption).
    pub chain_known: bool,
    /// The layout was inferred by skipping clusters that are now allocated
    /// to other files: a heuristic fragmented reconstruction.
    pub heuristic: bool,
    /// The recorded start of the file was untrustworthy (for example a
    /// FAT32 driver cleared the high word of the first cluster on deletion)
    /// and the start used here was inferred from free clusters and their
    /// content.
    pub start_inferred: bool,
}

impl Default for ExtentEvidence {
    fn default() -> Self {
        Self::known(false, false, 0, None)
    }
}

impl ExtentEvidence {
    /// Evidence for a stream whose layout is fully known.
    #[must_use]
    pub const fn known(
        resident: bool,
        complete: bool,
        extent_count: u32,
        total_clusters: Option<u64>,
    ) -> Self {
        Self {
            resident,
            complete,
            extent_count,
            total_clusters,
            expected_clusters: None,
            sparse: false,
            compressed: false,
            encrypted: false,
            chain_known: true,
            heuristic: false,
            start_inferred: false,
        }
    }
}

/// Evidence from the filesystem's allocation structures.
///
/// A cluster that is *allocated* has been assigned to active data; that
/// proves reuse, not that every previous byte is gone. User-facing wording
/// must preserve that distinction.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AllocationEvidence {
    /// Clusters examined.
    pub clusters_total: u64,
    /// Clusters currently free.
    pub clusters_free: u64,
    /// Clusters currently allocated to active data.
    pub clusters_allocated: u64,
    /// Clusters whose state could not be determined.
    pub clusters_unknown: u64,
    /// Whether an allocation map was available at all.
    pub map_available: bool,
}

impl AllocationEvidence {
    /// Fraction of examined clusters that are free, if any were examined.
    #[must_use]
    pub fn free_ratio(&self) -> Option<f64> {
        (self.clusters_total > 0).then(|| self.clusters_free as f64 / self.clusters_total as f64)
    }

    /// Fraction of examined clusters that are allocated, if any were examined.
    #[must_use]
    pub fn allocated_ratio(&self) -> Option<f64> {
        (self.clusters_total > 0)
            .then(|| self.clusters_allocated as f64 / self.clusters_total as f64)
    }
}

/// What zero-filled content means for this candidate.
///
/// Zeros are not evidence of loss by themselves: sparse files, zero-filled
/// binaries and pre-allocated files legitimately contain them. Whether they
/// count against recovery depends on what the content was expected to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ZeroContentAssessment {
    /// Zeros are expected here (sparse regions, empty files).
    Expected,
    /// Zeros are consistent with the recognised format or too few to matter.
    Plausible,
    /// A recognised type whose structure PHOINIX could not fully check reads
    /// mostly as zeros.
    Suspicious,
    /// The zeros violate the structure the file's type (detected or implied
    /// by its name) requires: strong negative evidence.
    ContradictsFormat,
    /// Unknown or raw content that is mostly zeros: wiped data and a
    /// legitimately zero-filled file look identical, so only the confidence
    /// is reduced.
    Ambiguous,
}

/// Evidence from the content itself.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ContentEvidence {
    /// Detected file type, if any signature matched.
    pub detected_type: Option<FileTypeDetection>,
    /// Type implied by the original filename's extension, if recognised.
    pub expected_type: Option<FileTypeDetection>,
    /// Structural validation result, if a validator ran.
    pub validation: Option<ValidationResult>,
    /// Fraction of sampled content blocks that were entirely zero, if the
    /// content was sampled.
    pub zero_block_ratio: Option<f64>,
    /// Bytes of content that were examined.
    pub bytes_examined: u64,
    /// Whether the first block of content (where a signature lives) is zero.
    pub head_is_zero: bool,
    /// Interpretation of any zero-filled content.
    pub zero_assessment: Option<ZeroContentAssessment>,
}

/// Kind of storage device the source lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceKind {
    /// A disk image file.
    Image,
    /// A physical or virtual block device.
    BlockDevice,
    /// Not known.
    #[default]
    Unknown,
}

/// Evidence about the storage medium.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StorageEvidence {
    /// Kind of source.
    pub device_kind: DeviceKind,
    /// Whether the medium is rotational, if known.
    pub rotational: Option<bool>,
    /// Whether TRIM/discard is supported, if known.
    pub trim_supported: Option<bool>,
    /// Whether the TRIM state of the freed clusters is actually known.
    pub trim_state_known: bool,
}

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// Informational.
    Info,
    /// Something reduces recoverability or certainty.
    Warning,
}

/// A free-form diagnostic carried alongside structured evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryDiagnostic {
    /// Severity.
    pub severity: DiagnosticSeverity,
    /// Message.
    pub message: String,
}

impl RecoveryDiagnostic {
    /// An informational diagnostic.
    #[must_use]
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Info,
            message: message.into(),
        }
    }

    /// A warning.
    #[must_use]
    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            message: message.into(),
        }
    }
}

/// How a candidate was discovered. The same underlying file may be found
/// through several sources; the scoring rules differ per source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateSource {
    /// A filesystem metadata record (MFT record, directory entry).
    #[default]
    FilesystemMetadata,
    /// A filesystem journal.
    Journal,
    /// Signature carving of unallocated (or all) space: no metadata.
    FileCarving,
    /// A reconstructed partition.
    PartitionReconstruction,
    /// A snapshot or shadow copy.
    Snapshot,
    /// Several sources merged.
    Combined,
}

/// Everything known about a candidate's recoverability.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RecoveryEvidence {
    /// How the candidate was discovered.
    #[serde(default)]
    pub source: CandidateSource,
    /// Metadata evidence.
    pub metadata: MetadataEvidence,
    /// Extent evidence.
    pub extents: ExtentEvidence,
    /// Allocation evidence.
    pub allocation: AllocationEvidence,
    /// Content evidence.
    pub content: ContentEvidence,
    /// Storage evidence.
    pub storage: StorageEvidence,
    /// Additional diagnostics.
    pub diagnostics: Vec<RecoveryDiagnostic>,
}
