//! Typed data transfer objects: the IPC contract between the service layer
//! and any front-end. Every type is plain data with a stable JSON form.

use std::path::PathBuf;

use phoinix_carve::CarveReport;
use phoinix_core::{CandidateId, FileSystemType};
use phoinix_fs::RecoveryCandidate;
use phoinix_health::{CandidateSource, HealthCategory};
use phoinix_partition_recovery::{PartitionCandidate, Repair};
use phoinix_recovery::RecoveryResult;
use serde::{Deserialize, Serialize};

/// A volume (partition or bare volume) of a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeInfo {
    /// 1-based partition index, or `None` for a bare volume.
    pub partition: Option<u32>,
    /// Byte offset inside the source.
    pub offset: u64,
    /// Length in bytes.
    pub length: u64,
    /// Partition type description (`NTFS / exFAT`, `Linux filesystem`, …).
    pub type_description: String,
    /// Detected filesystem.
    pub filesystem: FileSystemType,
    /// Probe confidence (0–100).
    pub confidence: u8,
    /// Whether PhoinixDR has an undelete engine for the filesystem.
    pub supported: bool,
    /// The volume came from the structure search rather than the table.
    #[serde(default)]
    pub lost: bool,
    /// Substitutions applied on mount (backup structures standing in for
    /// destroyed primaries), from the structure search.
    #[serde(default)]
    pub repairs: Vec<Repair>,
}

/// An explicit byte range of the source to use as the volume, typically
/// a lost partition found by the structure search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeRange {
    /// Byte offset inside the source.
    pub offset: u64,
    /// Length in bytes.
    pub length: u64,
    /// Repairs to overlay on mount.
    #[serde(default)]
    pub repairs: Vec<Repair>,
}

impl VolumeRange {
    /// The range of a partition candidate, repairs included.
    #[must_use]
    pub fn from_candidate(c: &PartitionCandidate) -> Self {
        Self {
            offset: c.start,
            length: c.readable_length,
            repairs: c.repairs.clone(),
        }
    }
}

/// Events emitted while the structure search runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SearchEvent {
    /// Bytes read so far.
    Progress {
        /// Bytes scanned.
        done: u64,
        /// Bytes in total.
        total: u64,
    },
    /// The search finished.
    Finished {
        /// Candidates in start order.
        candidates: Vec<PartitionCandidate>,
    },
    /// The search failed.
    Failed {
        /// Error text.
        message: String,
    },
}

/// What is known about a source before scanning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceInfo {
    /// Path as given.
    pub path: PathBuf,
    /// Whether the path is a block device (as opposed to an image file).
    pub is_device: bool,
    /// Size in bytes.
    pub size: u64,
    /// Logical sector size.
    pub sector_size: u32,
    /// Partition scheme (`GPT`, `MBR`, `None`).
    pub scheme: String,
    /// Volumes with their detected filesystems.
    pub volumes: Vec<VolumeInfo>,
    /// Partition-table diagnostics.
    pub diagnostics: Vec<String>,
}

/// Quick or deep scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ScanMode {
    /// Filesystem metadata only.
    #[default]
    Quick,
    /// Metadata plus signature carving of the unallocated space.
    Deep,
}

/// Carving options of a deep scan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CarveSettings {
    /// Carve the whole volume instead of the unallocated space.
    #[serde(default)]
    pub whole_volume: bool,
    /// Signature ids to carve (empty = all built-in).
    #[serde(default)]
    pub types: Vec<String>,
    /// Drop carved files shorter than this.
    #[serde(default)]
    pub min_size: u64,
    /// Alignment of tested offsets (0 = default 512).
    #[serde(default)]
    pub alignment: u64,
}

/// A scan request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRequest {
    /// Device path or image file.
    pub source: PathBuf,
    /// Partition index (default: first supported volume).
    #[serde(default)]
    pub partition: Option<u32>,
    /// An explicit volume range (a lost partition); takes precedence over
    /// `partition`.
    #[serde(default)]
    pub volume: Option<VolumeRange>,
    /// Quick or deep.
    #[serde(default)]
    pub mode: ScanMode,
    /// Whether content validators run (slower, higher confidence).
    #[serde(default = "default_true")]
    pub examine_content: bool,
    /// Carving options (deep scans).
    #[serde(default)]
    pub carve: CarveSettings,
}

const fn default_true() -> bool {
    true
}

/// A scan phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPhase {
    /// Opening the source and detecting the filesystem.
    Opening,
    /// Walking filesystem metadata.
    Metadata,
    /// Carving unallocated space.
    Carving,
    /// Deduplicating and finishing.
    Finishing,
}

/// A row of the results table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateSummary {
    /// Candidate identifier (stable within the session).
    pub id: CandidateId,
    /// Display name.
    pub name: String,
    /// Original path, if known.
    pub path: Option<String>,
    /// Whether the path is uncertain.
    pub path_uncertain: bool,
    /// Size in bytes, if known.
    pub size: Option<u64>,
    /// Health category.
    pub category: HealthCategory,
    /// Recovery likelihood (0–100).
    pub likelihood: u8,
    /// Assessment confidence (0–100).
    pub confidence: u8,
    /// How the candidate was found.
    pub source: CandidateSource,
    /// Detected or expected type id (`jpeg`, `docx`), if any.
    pub type_id: Option<String>,
    /// Human-readable type name.
    pub type_name: Option<String>,
    /// Modification time, ISO-8601 UTC.
    pub modified: Option<String>,
    /// Short reference (`64`, `c1048576`).
    pub reference: String,
}

impl CandidateSummary {
    /// Summarises a candidate.
    #[must_use]
    pub fn from_candidate(c: &RecoveryCandidate) -> Self {
        let t = c.evidence.content.detected_type.as_ref().or(c
            .evidence
            .content
            .expected_type
            .as_ref());
        Self {
            id: c.id,
            name: c.display_name(),
            path: c.original_path.clone(),
            path_uncertain: c.path_uncertain,
            size: c.logical_size,
            category: c.health.category,
            likelihood: c.health.likelihood,
            confidence: c.health.confidence,
            source: c.evidence.source,
            type_id: t.map(|t| t.id.clone()),
            type_name: t.map(|t| t.name.clone()),
            modified: c.timestamps.modified_iso.clone(),
            reference: c.filesystem_object.short_reference(),
        }
    }
}

/// Events emitted while a scan runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanEvent {
    /// The scan started.
    Started {
        /// Session identifier.
        session_id: String,
        /// Filesystem of the selected volume.
        filesystem: FileSystemType,
        /// Volume offset and length.
        volume: VolumeInfo,
    },
    /// A phase began.
    Phase {
        /// The phase.
        phase: ScanPhase,
    },
    /// Progress inside a phase.
    Progress {
        /// The phase.
        phase: ScanPhase,
        /// Units done (records, bytes).
        done: u64,
        /// Units in total, if known.
        total: Option<u64>,
        /// Candidates found so far.
        candidates: u64,
    },
    /// New candidates (batched).
    Candidates {
        /// The rows.
        items: Vec<CandidateSummary>,
    },
    /// The scan finished.
    Finished {
        /// Summary of the session.
        summary: SessionSummary,
    },
    /// The scan failed.
    Failed {
        /// Error text.
        message: String,
    },
    /// The scan was cancelled; partial results are kept.
    Cancelled {
        /// Summary of the partial session.
        summary: SessionSummary,
    },
}

/// Summary of a stored or in-memory session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    /// Session identifier.
    pub id: String,
    /// Where the session file lives, if saved.
    pub file: Option<PathBuf>,
    /// Source path.
    pub source: PathBuf,
    /// Partition index, if any.
    pub partition: Option<u32>,
    /// Filesystem of the volume.
    pub filesystem: FileSystemType,
    /// Quick or deep.
    pub mode: ScanMode,
    /// Unix seconds when the scan started.
    pub started: i64,
    /// Unix seconds when the scan finished, if it did.
    pub finished: Option<i64>,
    /// Whether the scan completed (not cancelled or failed).
    pub complete: bool,
    /// Number of candidates.
    pub candidates: usize,
    /// Candidates from filesystem metadata.
    pub from_metadata: usize,
    /// Carved candidates.
    pub carved: usize,
    /// Carving statistics, for deep scans.
    pub carving: Option<CarveReport>,
}

/// A recovery request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverRequest {
    /// Candidates to recover.
    pub candidates: Vec<CandidateId>,
    /// Destination directory.
    pub destination: PathBuf,
    /// Recreate the original directory tree.
    #[serde(default = "default_true")]
    pub preserve_tree: bool,
    /// Apply original timestamps.
    #[serde(default = "default_true")]
    pub preserve_timestamps: bool,
    /// Compute SHA-256 after writing.
    #[serde(default = "default_true")]
    pub hash: bool,
    /// Overwrite existing files instead of choosing a new name.
    #[serde(default)]
    pub overwrite: bool,
    /// Expert override for a destination on the source disk.
    #[serde(default)]
    pub allow_same_device: bool,
}

/// Outcome of one candidate's recovery.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoverItem {
    /// Candidate identifier.
    pub id: CandidateId,
    /// Display name.
    pub name: String,
    /// The result, on success.
    pub result: Option<RecoveryResult>,
    /// Error text, on failure.
    pub error: Option<String>,
}

/// Events emitted while recovering.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RecoverEvent {
    /// Recovery started.
    Started {
        /// Number of candidates.
        total: usize,
        /// Destination safety warning, if any.
        warning: Option<String>,
    },
    /// One candidate finished.
    Item {
        /// Position (1-based).
        index: usize,
        /// Number of candidates.
        total: usize,
        /// The outcome.
        item: RecoverItem,
    },
    /// All candidates processed.
    Finished {
        /// Outcomes.
        items: Vec<RecoverItem>,
        /// Number of failures or partial recoveries.
        failures: usize,
    },
}

/// Safety assessment of a destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationInfo {
    /// The destination path.
    pub destination: PathBuf,
    /// Whether it lies on the disk being recovered.
    pub same_disk: Option<bool>,
    /// Whether it would overwrite the source image.
    pub overwrites_source_image: bool,
    /// Whether writing there is refused without the expert override.
    pub dangerous: bool,
    /// Human-readable warning.
    pub warning: Option<String>,
}

/// A content preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Preview {
    /// An image the front-end can render (the webview decodes it).
    Image {
        /// MIME type.
        mime: String,
        /// Base64-encoded bytes.
        base64: String,
        /// Bytes encoded.
        bytes: u64,
    },
    /// Plain text.
    Text {
        /// The text (UTF-8, lossy).
        text: String,
        /// Whether the content was cut.
        truncated: bool,
    },
    /// A hex dump of the first bytes.
    Hex {
        /// The dump.
        dump: String,
        /// Bytes shown.
        bytes: u64,
    },
    /// Nothing could be shown.
    Unavailable {
        /// Why.
        reason: String,
    },
}
