//! The partition candidate model.

use std::sync::Arc;

use phoinix_block::{BlockError, BlockReader, Patch, PatchedReader, SubrangeReader};
use phoinix_core::ByteRange;
use phoinix_core::FileSystemType;
use phoinix_fs::ProbeEvidence;
use serde::{Deserialize, Serialize};

/// Which on-disk structure revealed the candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FoundVia {
    /// The primary boot sector at the volume start.
    PrimaryBootSector,
    /// A backup boot sector (NTFS: last sector; FAT32: sector 6; exFAT:
    /// sector 12); the primary is missing or damaged.
    BackupBootSector,
    /// The primary EXT superblock.
    Superblock,
    /// A backup EXT superblock in a later block group.
    BackupSuperblock,
}

/// How a candidate relates to the partition table and the other
/// candidates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Relation {
    /// Matches an entry of the partition table exactly.
    Listed {
        /// Partition index.
        index: u32,
    },
    /// No table entry covers it: a lost partition.
    Lost,
    /// Lies inside a table entry with different boundaries (a volume
    /// inside a partition, or a disk image stored on it).
    InsidePartition {
        /// Partition index.
        index: u32,
    },
    /// Lies entirely inside another candidate (typically an image file
    /// stored on that volume).
    Nested {
        /// Index of the enclosing candidate in the result list.
        within: usize,
    },
    /// Partially overlaps another candidate (stale structures of a former
    /// layout).
    Overlapping {
        /// Index of the overlapping candidate.
        with: usize,
    },
}

/// An in-memory substitution applied when the candidate is mounted: the
/// backup of a destroyed primary structure, placed where the primary
/// belongs. Nothing is written to the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repair {
    /// Offset inside the volume.
    pub offset: u64,
    /// Replacement bytes.
    pub bytes: Vec<u8>,
    /// What the repair does.
    pub description: String,
}

/// A volume found by its filesystem structures.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PartitionCandidate {
    /// Byte offset of the volume start inside the source.
    pub start: u64,
    /// Declared length in bytes (from the boot sector or superblock).
    pub length: u64,
    /// Bytes of the declared length that actually lie inside the source.
    pub readable_length: u64,
    /// Filesystem.
    pub filesystem: FileSystemType,
    /// Volume label, if any.
    pub label: Option<String>,
    /// Serial number or UUID, if any.
    pub serial: Option<String>,
    /// Cluster or block size in bytes.
    pub cluster_size: Option<u32>,
    /// Sector size declared by the structure.
    pub sector_size: u32,
    /// Which structure revealed it.
    pub found_via: FoundVia,
    /// The primary structure (boot sector / superblock) is valid.
    pub primary_structure_valid: bool,
    /// The backup structure is valid and matches, if one exists and could
    /// be read.
    pub backup_structure_valid: Option<bool>,
    /// The declared geometry fits the source and its sector size.
    pub geometry_consistent: bool,
    /// The filesystem engine opened the volume and read its root
    /// directory, if an engine exists.
    pub engine_verified: Option<bool>,
    /// Entries in the root directory, when the engine read it.
    pub root_entries: Option<usize>,
    /// Relation to the table and other candidates.
    pub relation: Relation,
    /// Substitutions applied on mount (backup structures standing in for
    /// destroyed primaries).
    #[serde(default)]
    pub repairs: Vec<Repair>,
    /// Evidence gathered.
    pub evidence: Vec<ProbeEvidence>,
    /// Confidence that this is a real, recoverable volume (0–100).
    pub confidence: u8,
}

impl PartitionCandidate {
    /// Exclusive end offset of the declared range.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.start.saturating_add(self.length)
    }

    /// The readable byte range inside the source.
    #[must_use]
    pub const fn byte_range(&self) -> ByteRange {
        ByteRange {
            offset: self.start,
            length: self.readable_length,
        }
    }

    /// Whether the candidate lies inside `other`.
    #[must_use]
    pub const fn inside(&self, other: &Self) -> bool {
        self.start >= other.start
            && self.end() <= other.end()
            && !(self.start == other.start && self.end() == other.end())
    }

    /// Whether the two ranges overlap.
    #[must_use]
    pub const fn overlaps(&self, other: &Self) -> bool {
        self.start < other.end() && other.start < self.end()
    }

    /// Opens the candidate as a volume (virtual mount): a read-only view of
    /// its readable range with the [`repairs`](Self::repairs) overlaid.
    /// Nothing is written to the partition table or the volume.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError`] if the range is outside the source.
    pub fn open(&self, source: Arc<dyn BlockReader>) -> Result<Arc<dyn BlockReader>, BlockError> {
        open_range(source, self.byte_range(), &self.repairs)
    }

    /// A one-line description.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "{} at {} ({} bytes){}",
            self.filesystem,
            self.start,
            self.length,
            self.label
                .as_ref()
                .map(|l| format!(", label {l:?}"))
                .unwrap_or_default()
        )
    }
}

/// Opens `range` of `source` with `repairs` overlaid.
///
/// # Errors
///
/// Returns [`BlockError`] if the range or a repair is outside the source.
pub fn open_range(
    source: Arc<dyn BlockReader>,
    range: ByteRange,
    repairs: &[Repair],
) -> Result<Arc<dyn BlockReader>, BlockError> {
    let view: Arc<dyn BlockReader> = Arc::new(SubrangeReader::new(source, range)?);
    if repairs.is_empty() {
        return Ok(view);
    }
    let patches = repairs
        .iter()
        .map(|r| Patch {
            offset: r.offset,
            bytes: r.bytes.clone(),
        })
        .collect();
    Ok(Arc::new(PatchedReader::new(view, patches)?))
}
