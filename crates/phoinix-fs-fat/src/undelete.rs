//! FAT deleted-file detection and evidence.

use std::io::Read;
use std::sync::Arc;

use phoinix_core::fmt::{grouped as group, iso8601_utc};
use phoinix_core::{CandidateId, SourceId};
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

use crate::FatError;
use crate::volume::{FatVolume, MAX_SKIPPED_CLUSTERS, WalkedEntry};

/// The FAT undelete engine.
pub struct FatUndelete {
    volume: Arc<FatVolume>,
    storage: StorageEvidence,
    model: ScoringModel,
    source_id: SourceId,
    examine_content: bool,
}

impl FatUndelete {
    /// Creates the engine.
    #[must_use]
    pub fn new(volume: Arc<FatVolume>, storage: StorageEvidence) -> Self {
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
    pub fn volume(&self) -> &FatVolume {
        &self.volume
    }

    fn is_candidate(w: &WalkedEntry) -> bool {
        w.entry.deleted
            && !w.entry.attributes.is_directory()
            && !w.entry.attributes.is_volume_label()
    }

    /// Builds the candidate for a walked entry.
    #[must_use]
    pub fn build_candidate(&self, w: &WalkedEntry) -> RecoveryCandidate {
        let entry = &w.entry;
        let mut diagnostics = Vec::new();
        if entry.long_name.is_none() {
            diagnostics.push(RecoveryDiagnostic::warning(
                "The first character of the short name was lost on deletion and is shown as '?'",
            ));
        } else if entry.long_name_unverified {
            diagnostics.push(RecoveryDiagnostic::info("The long name was taken from deleted entries whose checksum can no longer be verified"));
        }
        let cs = u64::from(self.volume.cluster_size());
        let needed = u64::from(entry.size).div_ceil(cs);
        let reconstruction = self.volume.reconstruct(entry);
        let inferred = reconstruction
            .as_ref()
            .ok()
            .and_then(|r| r.inferred_start.as_ref());
        if let Some(i) = inferred {
            let recorded_state = if i.recorded_allocated {
                "is allocated to other data"
            } else {
                "holds no plausible content"
            };
            diagnostics.push(RecoveryDiagnostic::warning(format!(
                "The high word of the first cluster was cleared on deletion and the recorded cluster {} {}; cluster {} was chosen among {} free candidate{} sharing the low word because {}",
                group(u64::from(i.recorded)),
                recorded_state,
                group(u64::from(i.chosen)),
                group(u64::from(i.candidates)),
                if i.candidates == 1 { "" } else { "s" },
                i.evidence.describe()
            )));
        } else if self.volume.high_word_untrustworthy(entry) && needed > 0 {
            diagnostics.push(RecoveryDiagnostic::warning(
                "The high word of the first cluster is zero; Windows clears it on deletion, so the start of the file may be wrong on this large volume. No free cluster sharing the low word held more plausible content",
            ));
        }
        if reconstruction.as_ref().is_ok_and(|r| r.search_exhausted) {
            diagnostics.push(RecoveryDiagnostic::warning(format!(
                "No free cluster was found within {} clusters after the assumed start; that region is fully allocated, so the recorded start is probably wrong",
                group(MAX_SKIPPED_CLUSTERS as u64)
            )));
        }
        if w.via_deleted_directory {
            diagnostics.push(RecoveryDiagnostic::info(
                "The original path passes through a deleted directory whose name survived",
            ));
        }
        let (extents, allocation) = match &reconstruction {
            Ok(r) => {
                let span: Vec<u32> = if r.chain_known {
                    r.clusters.clone()
                } else {
                    r.contiguous_span.clone()
                };
                let fat = self.volume.fat();
                let free = span.iter().filter(|c| fat.is_free(**c)).count() as u64;
                let allocation = AllocationEvidence {
                    clusters_total: span.len() as u64,
                    clusters_free: free,
                    clusters_allocated: span.len() as u64 - free,
                    clusters_unknown: 0,
                    map_available: true,
                };
                let extents = ExtentEvidence {
                    resident: false,
                    complete: r.complete,
                    extent_count: r.extent_count,
                    total_clusters: Some(r.clusters.len() as u64),
                    expected_clusters: Some(u64::from(entry.size).div_ceil(cs)),
                    sparse: false,
                    compressed: false,
                    encrypted: false,
                    chain_known: r.chain_known,
                    heuristic: r.is_heuristic(),
                    start_inferred: r.inferred_start.is_some(),
                    stale: false,
                    unreadable_bytes: 0,
                };
                (extents, allocation)
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
                        expected_clusters: Some(needed),
                        ..Default::default()
                    },
                    AllocationEvidence {
                        map_available: true,
                        ..Default::default()
                    },
                )
            }
        };
        let metadata = MetadataEvidence {
            valid_record: true,
            filename_available: true,
            original_parent_available: true,
            parent_reference_valid: true,
            logical_size_available: true,
            logical_size: Some(u64::from(entry.size)),
            timestamps_available: entry.modified.is_some() || entry.created.is_some(),
        };
        let name = entry.name().to_owned();
        let expected_type = expected_type_from_name(&name);
        let mut content = ContentEvidence::default();
        // Content is examined when requested; zero sampling always runs.
        if extents.complete && entry.size > 0 && reconstruction.is_ok() {
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
            filesystem: self.volume.variant().filesystem_type(),
            filesystem_object: FileSystemObjectId::Fat {
                entry_offset: entry.entry_offset,
            },
            original_name: Some(name),
            original_path: Some(w.path.clone()),
            path_uncertain: false,
            logical_size: Some(u64::from(entry.size)),
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

    fn find(&self, entry_offset: u64) -> Result<WalkedEntry, FatError> {
        self.volume
            .walk()?
            .into_iter()
            .find(|w| w.entry.entry_offset == entry_offset && Self::is_candidate(w))
            .ok_or_else(|| {
                FatError::NotFound(format!("no deleted FAT entry at offset {entry_offset}"))
            })
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

impl DeletedFileProvider for FatUndelete {
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
            FileSystemObjectId::Fat { entry_offset } => {
                Ok(self.build_candidate(&self.find(*entry_offset)?))
            }
            other => Err(FsError::NotFound(format!("{other} is not a FAT object"))),
        }
    }

    fn object_from_reference(&self, text: &str) -> Result<FileSystemObjectId, FsError> {
        let entry_offset: u64 = text.trim().parse().map_err(|_| {
            FsError::NotFound(format!(
                "invalid FAT candidate reference {text:?}; expected a directory entry offset"
            ))
        })?;
        Ok(FileSystemObjectId::Fat { entry_offset })
    }

    fn open_content(
        &self,
        candidate: &RecoveryCandidate,
    ) -> Result<Box<dyn CandidateContent>, FsError> {
        let FileSystemObjectId::Fat { entry_offset } = &candidate.filesystem_object else {
            return Err(FsError::NotFound(format!(
                "{} is not a FAT object",
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
        let FileSystemObjectId::Fat { entry_offset } = &candidate.filesystem_object else {
            return Err(FsError::NotFound(format!(
                "{} is not a FAT object",
                candidate.filesystem_object
            )));
        };
        let w = self.find(*entry_offset)?;
        Ok(self.volume.open_stream(&w.entry)?.extents().to_vec())
    }
}

impl AllocationView for FatUndelete {
    fn cluster_size(&self) -> u64 {
        u64::from(self.volume.cluster_size().max(1))
    }

    fn volume_len(&self) -> u64 {
        self.volume.boot().volume_bytes()
    }

    fn map_available(&self) -> bool {
        true
    }

    fn free_ranges(&self) -> Result<Vec<ByteRange>, FsError> {
        let boot = self.volume.boot();
        let fat = self.volume.fat();
        Ok(phoinix_fs::space::free_ranges_from(
            u64::from(boot.cluster_count),
            AllocationView::cluster_size(self),
            boot.data_offset,
            |c| {
                u32::try_from(c.saturating_add(2))
                    .ok()
                    .map(|n| fat.is_free(n))
            },
        ))
    }

    fn summarize(&self, range: ByteRange) -> AllocationSummary {
        let boot = self.volume.boot();
        let fat = self.volume.fat();
        phoinix_fs::space::summarize_with(
            range,
            AllocationView::cluster_size(self),
            boot.data_offset,
            u64::from(boot.cluster_count),
            |c| {
                u32::try_from(c.saturating_add(2))
                    .ok()
                    .map(|n| fat.is_free(n))
            },
        )
    }
}
