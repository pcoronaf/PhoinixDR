//! exFAT deleted-file detection and evidence.

use std::io::Read;
use std::sync::Arc;

use phoinix_core::fmt::iso8601_utc;
use phoinix_core::{CandidateId, FileSystemType, SourceId};
use phoinix_fs::{
    AllocationSummary, AllocationView, ByteRange, CandidateContent, CandidateTimestamps,
    DeletedFileProvider, Extent, ExtentStreamCursor, FileSystemObjectId, FsError,
    RecoveryCandidate,
};
use phoinix_health::validate::{
    DEFAULT_BYTE_BUDGET, ZERO_SAMPLE_BLOCKS, assess_zero_content, examine, expected_type_from_name,
    sample_content,
};
use phoinix_health::{
    AllocationEvidence, CandidateSource, ContentEvidence, ExtentEvidence, MetadataEvidence,
    RecoveryDiagnostic, RecoveryEvidence, ScoringModel, StorageEvidence, score,
};

use crate::ExfatError;
use crate::bitmap::ClusterState;
use crate::dir::EntrySet;
use crate::volume::{ExfatVolume, WalkedEntry};

/// The exFAT undelete engine.
pub struct ExfatUndelete {
    volume: Arc<ExfatVolume>,
    storage: StorageEvidence,
    model: ScoringModel,
    source_id: SourceId,
    examine_content: bool,
}

impl ExfatUndelete {
    /// Creates the engine.
    #[must_use]
    pub fn new(volume: Arc<ExfatVolume>, storage: StorageEvidence) -> Self {
        let source_id = volume.reader().id();
        Self {
            volume,
            storage,
            model: ScoringModel::default(),
            source_id,
            examine_content: true,
        }
    }

    /// Disables content examination.
    #[must_use]
    pub const fn without_content_examination(mut self) -> Self {
        self.examine_content = false;
        self
    }

    /// The volume.
    #[must_use]
    pub fn volume(&self) -> &ExfatVolume {
        &self.volume
    }

    fn is_candidate(w: &WalkedEntry) -> bool {
        w.entry.deleted && !w.entry.is_directory()
    }

    /// Builds the candidate for a walked entry.
    #[must_use]
    pub fn build_candidate(&self, w: &WalkedEntry) -> RecoveryCandidate {
        let entry = &w.entry;
        let mut diagnostics = Vec::new();
        if !entry.checksum_ok {
            diagnostics.push(RecoveryDiagnostic::warning(
                "The directory entry set checksum does not match; some fields may be damaged",
            ));
        }
        if w.via_deleted_directory {
            diagnostics.push(RecoveryDiagnostic::info(
                "The original path passes through a deleted directory whose name survived",
            ));
        }
        let reconstruction = self.volume.reconstruct(entry);
        let (extents, allocation, clusters) = match &reconstruction {
            Ok(r) => {
                let span = if r.chain_known {
                    &r.clusters
                } else {
                    &r.contiguous_span
                };
                let allocation = match self.volume.bitmap() {
                    Some(b) => {
                        let s = b.summarize(span.iter().copied());
                        AllocationEvidence {
                            clusters_total: s.free + s.allocated + s.unknown,
                            clusters_free: s.free,
                            clusters_allocated: s.allocated,
                            clusters_unknown: s.unknown,
                            map_available: true,
                        }
                    }
                    None => AllocationEvidence {
                        clusters_total: span.len() as u64,
                        clusters_unknown: span.len() as u64,
                        map_available: false,
                        ..Default::default()
                    },
                };
                let heuristic = r.assumed_contiguous && !r.skipped_allocated.is_empty();
                if r.assumed_contiguous && !heuristic {
                    diagnostics.push(RecoveryDiagnostic::warning(
                        "The FAT chain is gone; the file is assumed contiguous",
                    ));
                }
                let extents = ExtentEvidence {
                    resident: false,
                    complete: r.complete,
                    extent_count: r.extent_count,
                    total_clusters: Some(r.clusters.len() as u64),
                    expected_clusters: Some(
                        entry
                            .data_length
                            .div_ceil(u64::from(self.volume.cluster_size())),
                    ),
                    sparse: false,
                    compressed: false,
                    encrypted: false,
                    chain_known: r.chain_known,
                    heuristic,
                    start_inferred: false,
                    stale: false,
                    unreadable_bytes: 0,
                };
                (extents, allocation, r.clusters.clone())
            }
            Err(e) => {
                diagnostics.push(RecoveryDiagnostic::warning(format!(
                    "Clusters could not be determined: {e}"
                )));
                (
                    ExtentEvidence {
                        complete: false,
                        chain_known: false,
                        total_clusters: Some(0),
                        expected_clusters: Some(
                            entry
                                .data_length
                                .div_ceil(u64::from(self.volume.cluster_size())),
                        ),
                        ..Default::default()
                    },
                    AllocationEvidence {
                        map_available: self.volume.bitmap().is_some(),
                        ..Default::default()
                    },
                    Vec::new(),
                )
            }
        };
        let _ = clusters;
        let metadata = MetadataEvidence {
            valid_record: entry.checksum_ok,
            filename_available: !entry.name.is_empty(),
            original_parent_available: true,
            parent_reference_valid: true,
            logical_size_available: true,
            logical_size: Some(entry.data_length),
            timestamps_available: entry.modified.is_some() || entry.created.is_some(),
        };
        let expected_type = expected_type_from_name(&entry.name);
        let mut content = ContentEvidence::default();
        // Content is examined when requested; zero sampling always runs.
        if extents.complete && entry.data_length > 0 && reconstruction.is_ok() {
            match self.volume.open_stream(entry) {
                Ok(s) => {
                    let mut cursor = s.cursor();
                    match if self.examine_content {
                        examine(&mut cursor, s.len(), DEFAULT_BYTE_BUDGET)
                    } else {
                        sample_content(&mut cursor, s.len(), ZERO_SAMPLE_BLOCKS)
                    } {
                        Ok(mut c) => {
                            c.zero_assessment = assess_zero_content(
                                c.zero_block_ratio.unwrap_or(0.0),
                                c.head_is_zero,
                                false,
                                c.detected_type.as_ref(),
                                expected_type.as_ref(),
                                c.validation.as_ref(),
                            );
                            content = c;
                        }
                        Err(e) => diagnostics.push(RecoveryDiagnostic::warning(format!(
                            "Content could not be examined: {e}"
                        ))),
                    }
                }
                Err(e) => diagnostics.push(RecoveryDiagnostic::warning(format!(
                    "Content could not be opened: {e}"
                ))),
            }
        }
        content.expected_type = expected_type;
        let evidence = RecoveryEvidence {
            source: CandidateSource::FilesystemMetadata,
            metadata,
            extents,
            allocation,
            content,
            storage: self.storage.clone(),
            diagnostics,
        };
        let health = score(&evidence, &self.model);
        let iso = |t: Option<i64>| t.map(|s| iso8601_utc(s, 0));
        RecoveryCandidate {
            id: CandidateId::new(),
            source_id: self.source_id,
            filesystem: FileSystemType::ExFat,
            filesystem_object: FileSystemObjectId::ExFat {
                entry_offset: entry.entry_offset,
            },
            original_name: Some(entry.name.clone()),
            original_path: Some(w.path.clone()),
            path_uncertain: false,
            logical_size: Some(entry.data_length),
            deleted: true,
            timestamps: CandidateTimestamps {
                created: entry.created,
                modified: entry.modified,
                accessed: entry.accessed,
                created_iso: iso(entry.created),
                modified_iso: iso(entry.modified),
                accessed_iso: iso(entry.accessed),
            },
            evidence,
            health,
        }
    }

    fn find(&self, entry_offset: u64) -> Result<WalkedEntry, ExfatError> {
        self.volume
            .walk()?
            .into_iter()
            .find(|w| w.entry.entry_offset == entry_offset && Self::is_candidate(w))
            .ok_or_else(|| {
                ExfatError::NotFound(format!("no deleted exFAT entry at offset {entry_offset}"))
            })
    }

    /// Allocation state of a cluster (for tests and explanations).
    #[must_use]
    pub fn cluster_state(&self, cluster: u32) -> ClusterState {
        self.volume.cluster_state(cluster)
    }
}

struct Content {
    cursor: ExtentStreamCursor,
}

impl Read for Content {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}

impl CandidateContent for Content {
    fn len(&self) -> u64 {
        self.cursor.stream().len()
    }
}

impl DeletedFileProvider for ExfatUndelete {
    fn deleted_files(&self) -> Box<dyn Iterator<Item = Result<RecoveryCandidate, FsError>> + '_> {
        match self.volume.walk() {
            Ok(entries) => Box::new(
                entries
                    .into_iter()
                    .filter(Self::is_candidate)
                    .map(move |w| Ok(self.build_candidate(&w))),
            ),
            Err(e) => Box::new(std::iter::once(Err(e.into()))),
        }
    }

    fn candidate(&self, object: &FileSystemObjectId) -> Result<RecoveryCandidate, FsError> {
        match object {
            FileSystemObjectId::ExFat { entry_offset } => {
                Ok(self.build_candidate(&self.find(*entry_offset)?))
            }
            other => Err(FsError::NotFound(format!("{other} is not an exFAT object"))),
        }
    }

    fn object_from_reference(&self, text: &str) -> Result<FileSystemObjectId, FsError> {
        let entry_offset: u64 = text.trim().parse().map_err(|_| {
            FsError::NotFound(format!(
                "invalid exFAT candidate reference {text:?}; expected a directory entry offset"
            ))
        })?;
        Ok(FileSystemObjectId::ExFat { entry_offset })
    }

    fn open_content(
        &self,
        candidate: &RecoveryCandidate,
    ) -> Result<Box<dyn CandidateContent>, FsError> {
        let FileSystemObjectId::ExFat { entry_offset } = &candidate.filesystem_object else {
            return Err(FsError::NotFound(format!(
                "{} is not an exFAT object",
                candidate.filesystem_object
            )));
        };
        let w = self.find(*entry_offset)?;
        let stream = self.volume.open_stream(&w.entry)?;
        Ok(Box::new(Content {
            cursor: stream.cursor(),
        }))
    }

    fn content_extents(&self, candidate: &RecoveryCandidate) -> Result<Vec<Extent>, FsError> {
        let FileSystemObjectId::ExFat { entry_offset } = &candidate.filesystem_object else {
            return Err(FsError::NotFound(format!(
                "{} is not an exFAT object",
                candidate.filesystem_object
            )));
        };
        let w = self.find(*entry_offset)?;
        Ok(self.volume.open_stream(&w.entry)?.extents().to_vec())
    }
}

impl AllocationView for ExfatUndelete {
    fn cluster_size(&self) -> u64 {
        u64::from(self.volume.cluster_size().max(1))
    }

    fn volume_len(&self) -> u64 {
        self.volume.boot().volume_bytes()
    }

    fn map_available(&self) -> bool {
        self.volume.bitmap().is_some()
    }

    fn free_ranges(&self) -> Result<Vec<ByteRange>, FsError> {
        let boot = self.volume.boot();
        Ok(phoinix_fs::space::free_ranges_from(
            u64::from(boot.cluster_count),
            AllocationView::cluster_size(self),
            boot.heap_offset,
            |c| self.cluster_free(c),
        ))
    }

    fn summarize(&self, range: ByteRange) -> AllocationSummary {
        let boot = self.volume.boot();
        phoinix_fs::space::summarize_with(
            range,
            AllocationView::cluster_size(self),
            boot.heap_offset,
            u64::from(boot.cluster_count),
            |c| self.cluster_free(c),
        )
    }
}

impl ExfatUndelete {
    /// Free state of the 0-based heap cluster `index` (cluster `index + 2`).
    fn cluster_free(&self, index: u64) -> Option<bool> {
        let n = u32::try_from(index.saturating_add(2)).ok()?;
        match self.volume.cluster_state(n) {
            ClusterState::Free => Some(true),
            ClusterState::Allocated => Some(false),
            ClusterState::Unknown => None,
        }
    }
}

#[allow(dead_code)]
fn _entry_type_check(_: &EntrySet) {}
