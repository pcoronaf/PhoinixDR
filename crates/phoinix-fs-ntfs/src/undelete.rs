//! Deleted-file detection and evidence gathering (M4).
//!
//! A primary candidate is a valid FILE record whose *in use* flag is clear
//! and that still carries useful `$FILE_NAME` and/or `$DATA` attributes.
//! For every data stream of such a record PHOINIX gathers:
//!
//! - metadata evidence (name, parent validity, size, timestamps);
//! - extent evidence (resident, runlist completeness, fragmentation);
//! - allocation evidence from `$Bitmap` — clusters *allocated to active
//!   data*, never "overwritten";
//! - content evidence from the structural validators and zero sampling;
//! - storage evidence supplied by the caller.
//!
//! The evidence is scored by `phoinix-health` into a
//! [`RecoveryHealth`].

use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use phoinix_core::{CandidateId, FileSystemType, SourceId};
use phoinix_fs::{
    CandidateContent, CandidateTimestamps, DeletedFileProvider, FileSystemObjectId, FsError,
    RecoveryCandidate,
};
use phoinix_health::validate::{
    DEFAULT_BYTE_BUDGET, assess_zero_content, examine, expected_type_from_name,
};
use phoinix_health::{
    AllocationEvidence, ContentEvidence, ExtentEvidence, MetadataEvidence, RecoveryDiagnostic,
    RecoveryEvidence, RecoveryHealth, ScoringModel, StorageEvidence, score,
};
use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::bitmap::{ClusterAllocationMap, ClusterBitmap, RangeAllocation};
use crate::data::{DataStorage, DataStreamDescriptor};
use crate::diagnostic::NtfsDiagnostic;
use crate::file::NtfsFile;
use crate::filename::FileNameAttribute;
use crate::runlist::NtfsRun;
use crate::stream::StreamCursor;
use crate::tree::ResolvedPath;
use crate::volume::NtfsVolume;

/// How much of a deleted record's metadata survived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateMetadataState {
    /// Name, parent, size and data stream all present.
    Complete,
    /// Name and data present but the parent is stale or the path uncertain.
    Partial,
    /// Data runs exist but no filename survived.
    Minimal,
    /// The record parsed but its attributes are damaged.
    Corrupt,
}

/// A deleted NTFS record with its surviving attributes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NtfsDeletedCandidate {
    /// The record.
    pub record: crate::record::FileReference,
    /// Surviving names.
    pub names: Vec<FileNameAttribute>,
    /// Surviving data streams.
    pub streams: Vec<DataStreamDescriptor>,
    /// Whether the record was a directory.
    pub directory: bool,
    /// Reconstructed path.
    pub path: ResolvedPath,
    /// Metadata completeness.
    pub metadata_state: CandidateMetadataState,
    /// Findings.
    pub diagnostics: Vec<NtfsDiagnostic>,
}

/// The NTFS undelete engine over one volume.
pub struct NtfsUndelete {
    volume: Arc<NtfsVolume>,
    bitmap: Option<ClusterBitmap>,
    storage: StorageEvidence,
    model: ScoringModel,
    source_id: SourceId,
    byte_budget: u64,
    examine_content: bool,
}

impl NtfsUndelete {
    /// Creates the engine, loading `$Bitmap`. If the bitmap cannot be read,
    /// candidates are still produced with unknown allocation state.
    #[must_use]
    pub fn new(volume: Arc<NtfsVolume>, storage: StorageEvidence) -> Self {
        let bitmap = match ClusterBitmap::load(&volume) {
            Ok(b) => Some(b),
            Err(e) => {
                tracing::warn!(error = %e, "$Bitmap unavailable; allocation evidence will be unknown");
                None
            }
        };
        let source_id = volume.reader().id();
        Self {
            volume,
            bitmap,
            storage,
            model: ScoringModel::default(),
            source_id,
            byte_budget: DEFAULT_BYTE_BUDGET,
            examine_content: true,
        }
    }

    /// Uses a custom scoring model.
    #[must_use]
    pub fn with_model(mut self, model: ScoringModel) -> Self {
        self.model = model;
        self
    }

    /// Limits how many bytes of content validators may read per candidate.
    #[must_use]
    pub const fn with_byte_budget(mut self, budget: u64) -> Self {
        self.byte_budget = budget;
        self
    }

    /// Disables content examination (faster scans; lower confidence).
    #[must_use]
    pub const fn without_content_examination(mut self) -> Self {
        self.examine_content = false;
        self
    }

    /// The volume.
    #[must_use]
    pub fn volume(&self) -> &NtfsVolume {
        &self.volume
    }

    /// The loaded bitmap, if any.
    #[must_use]
    pub const fn bitmap(&self) -> Option<&ClusterBitmap> {
        self.bitmap.as_ref()
    }

    /// Whether `file` is a deleted record worth reporting.
    #[must_use]
    pub fn is_candidate(file: &NtfsFile) -> bool {
        !file.in_use
            && file.is_base
            && !file.directory
            && (!file.names.is_empty() || !file.streams.is_empty())
    }

    /// Classifies surviving metadata.
    #[must_use]
    pub fn metadata_state(file: &NtfsFile, path: &ResolvedPath) -> CandidateMetadataState {
        let has_name = file.name().is_some();
        let has_data = !file.streams.is_empty();
        let damaged = file.diagnostics.iter().any(|d| {
            matches!(
                d,
                NtfsDiagnostic::AttributeError { .. } | NtfsDiagnostic::RunlistError { .. }
            )
        });
        if damaged && !(has_name && has_data) {
            CandidateMetadataState::Corrupt
        } else if has_name && has_data && !path.uncertain {
            CandidateMetadataState::Complete
        } else if has_name {
            CandidateMetadataState::Partial
        } else {
            CandidateMetadataState::Minimal
        }
    }

    /// Enumerates deleted records as [`NtfsDeletedCandidate`]s.
    pub fn deleted_candidates(
        &self,
    ) -> impl Iterator<Item = Result<NtfsDeletedCandidate, NtfsError>> + '_ {
        let resolver = self.volume.resolver();
        self.volume
            .files()
            .filter_map(move |(number, result)| match result {
                Ok(file) if Self::is_candidate(&file) => {
                    let path = resolver.resolve(&file);
                    let metadata_state = Self::metadata_state(&file, &path);
                    Some(Ok(NtfsDeletedCandidate {
                        record: file.reference,
                        names: file.names,
                        streams: file.streams,
                        directory: file.directory,
                        path,
                        metadata_state,
                        diagnostics: file.diagnostics,
                    }))
                }
                Ok(_) => None,
                Err(e) => {
                    tracing::trace!(record = number, error = %e, "record skipped");
                    None
                }
            })
    }

    /// Builds every [`RecoveryCandidate`] (one per data stream) of a file.
    #[must_use]
    pub fn candidates_for_file(
        &self,
        file: &NtfsFile,
        path: &ResolvedPath,
    ) -> Vec<RecoveryCandidate> {
        if file.streams.is_empty() {
            return vec![self.build_candidate(file, path, None)];
        }
        let mut out: Vec<RecoveryCandidate> = file
            .streams
            .iter()
            .map(|s| self.build_candidate(file, path, Some(s)))
            .collect();
        // Unnamed stream first.
        out.sort_by_key(|c| match &c.filesystem_object {
            FileSystemObjectId::Ntfs { stream, .. } => stream.clone(),
            _ => None,
        });
        out
    }

    /// Builds the candidate for one stream of a deleted file.
    #[must_use]
    pub fn build_candidate(
        &self,
        file: &NtfsFile,
        path: &ResolvedPath,
        stream: Option<&DataStreamDescriptor>,
    ) -> RecoveryCandidate {
        let evidence = self.evidence(file, path, stream);
        let health = if stream.is_some() {
            score(&evidence, &self.model)
        } else {
            RecoveryHealth::unknown("The record has no data stream")
        };
        let name = file
            .name()
            .map(|n| match stream.and_then(|s| s.name.as_deref()) {
                Some(ads) => format!("{n}:{ads}"),
                None => n.to_owned(),
            });
        RecoveryCandidate {
            id: CandidateId::new(),
            source_id: self.source_id,
            filesystem: FileSystemType::Ntfs,
            filesystem_object: FileSystemObjectId::Ntfs {
                record: file.reference.record,
                sequence: file.reference.sequence,
                stream: stream.and_then(|s| s.name.clone()),
            },
            original_name: name,
            original_path: file.name().map(|_| path.path.clone()),
            path_uncertain: path.uncertain,
            logical_size: stream.map(|s| s.logical_size),
            deleted: !file.in_use,
            timestamps: timestamps(file),
            evidence,
            health,
        }
    }

    fn evidence(
        &self,
        file: &NtfsFile,
        path: &ResolvedPath,
        stream: Option<&DataStreamDescriptor>,
    ) -> RecoveryEvidence {
        let parent_problem = path.diagnostics.iter().any(|d| {
            matches!(
                d,
                NtfsDiagnostic::ParentUnreadable { .. }
                    | NtfsDiagnostic::ParentUnnamed { .. }
                    | NtfsDiagnostic::NoFileName
                    | NtfsDiagnostic::PathLoop
                    | NtfsDiagnostic::PathDepthExceeded
            )
        });
        let metadata = MetadataEvidence {
            valid_record: !file
                .diagnostics
                .iter()
                .any(|d| matches!(d, NtfsDiagnostic::AttributeError { .. })),
            filename_available: file.name().is_some(),
            original_parent_available: file.name().is_some() && !parent_problem,
            parent_reference_valid: !path.uncertain,
            logical_size_available: stream.is_some(),
            logical_size: stream.map(|s| s.logical_size),
            timestamps_available: file.standard_information.is_some() || !file.names.is_empty(),
        };

        let mut diagnostics: Vec<RecoveryDiagnostic> = Vec::new();
        for d in &file.diagnostics {
            match d {
                NtfsDiagnostic::UnknownAttribute { .. } => {}
                other => diagnostics.push(RecoveryDiagnostic::warning(other.to_string())),
            }
        }
        for d in &path.diagnostics {
            match d {
                NtfsDiagnostic::ParentDeleted { .. } => {
                    if !diagnostics
                        .iter()
                        .any(|x| x.message.contains("deleted directory"))
                    {
                        diagnostics.push(RecoveryDiagnostic::info("The original path passes through a deleted directory whose name survived"));
                    }
                }
                other => diagnostics.push(RecoveryDiagnostic::warning(other.to_string())),
            }
        }

        let Some(stream) = stream else {
            return RecoveryEvidence {
                metadata,
                extents: ExtentEvidence::default(),
                allocation: AllocationEvidence {
                    map_available: self.bitmap.is_some(),
                    ..Default::default()
                },
                content: ContentEvidence::default(),
                storage: self.storage.clone(),
                diagnostics,
            };
        };

        let cluster = u64::from(self.volume.cluster_size().max(1));
        let (resident, complete, compressed, encrypted, expected_clusters) = match &stream.storage {
            DataStorage::Resident { .. } => (true, true, false, false, None),
            DataStorage::NonResident {
                complete,
                allocated_size,
                ..
            } => (
                false,
                *complete,
                false,
                false,
                Some(
                    allocated_size
                        .div_ceil(cluster)
                        .max(stream.logical_size.div_ceil(cluster)),
                ),
            ),
            DataStorage::UnsupportedCompressed { .. } => (false, true, true, false, None),
            DataStorage::UnsupportedEncrypted { .. } => (false, true, false, true, None),
        };
        let extents = ExtentEvidence {
            resident,
            complete,
            extent_count: stream.extent_count(),
            total_clusters: if resident {
                None
            } else {
                Some(stream.data_clusters())
            },
            expected_clusters,
            sparse: stream.has_sparse_runs() || stream.flags & crate::attribute::FLAG_SPARSE != 0,
            chain_known: true,
            heuristic: false,
            start_inferred: false,
            compressed,
            encrypted,
        };

        let allocation = if resident {
            AllocationEvidence {
                map_available: true,
                ..Default::default()
            }
        } else {
            match &self.bitmap {
                Some(bitmap) => {
                    let mut total = RangeAllocation::default();
                    for run in stream.storage.runs() {
                        if let NtfsRun::Data { lcn, clusters, .. } = run {
                            total.add(bitmap.summarize(*lcn, *clusters));
                        }
                    }
                    AllocationEvidence {
                        clusters_total: total.total(),
                        clusters_free: total.free,
                        clusters_allocated: total.allocated,
                        clusters_unknown: total.unknown,
                        map_available: true,
                    }
                }
                None => AllocationEvidence {
                    clusters_total: stream.data_clusters(),
                    clusters_unknown: stream.data_clusters(),
                    map_available: false,
                    ..Default::default()
                },
            }
        };

        let expected_type = file.name().and_then(expected_type_from_name);
        let mut content = if self.examine_content
            && stream.storage.is_readable()
            && complete
            && stream.logical_size > 0
        {
            match self.volume.open_stream(file, stream.name.as_deref()) {
                Ok(s) => {
                    let mut cursor = s.cursor();
                    match examine(&mut cursor, s.len(), self.byte_budget) {
                        Ok(mut c) => {
                            c.zero_assessment = assess_zero_content(
                                c.zero_block_ratio.unwrap_or(0.0),
                                c.head_is_zero,
                                extents.sparse,
                                c.detected_type.as_ref(),
                                expected_type.as_ref(),
                                c.validation.as_ref(),
                            );
                            c
                        }
                        Err(e) => {
                            diagnostics.push(RecoveryDiagnostic::warning(format!(
                                "Content could not be examined: {e}"
                            )));
                            ContentEvidence::default()
                        }
                    }
                }
                Err(e) => {
                    diagnostics.push(RecoveryDiagnostic::warning(format!(
                        "Content could not be opened: {e}"
                    )));
                    ContentEvidence::default()
                }
            }
        } else {
            ContentEvidence::default()
        };

        content.expected_type = expected_type;

        RecoveryEvidence {
            metadata,
            extents,
            allocation,
            content,
            storage: self.storage.clone(),
            diagnostics,
        }
    }

    /// Rebuilds the candidate for a record and stream.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::NotFound`] if the record is not a deleted file
    /// or the stream does not exist.
    pub fn candidate_for(
        &self,
        record: u64,
        stream: Option<&str>,
    ) -> Result<RecoveryCandidate, NtfsError> {
        let file = self.volume.file(record)?;
        if !Self::is_candidate(&file) {
            return Err(NtfsError::NotFound(format!(
                "record {record} is not a deleted file"
            )));
        }
        let path = self.volume.resolver().resolve(&file);
        if file.streams.is_empty() && stream.is_none() {
            return Ok(self.build_candidate(&file, &path, None));
        }
        let descriptor = file
            .stream(stream)
            .ok_or_else(|| NtfsError::NotFound(format!("stream {stream:?} of record {record}")))?;
        Ok(self.build_candidate(&file, &path, Some(descriptor)))
    }
}

fn timestamps(file: &NtfsFile) -> CandidateTimestamps {
    let (c, m, a) = match (&file.standard_information, file.preferred_name()) {
        (Some(si), _) => (si.created, si.modified, si.accessed),
        (None, Some(n)) => (n.created, n.modified, n.accessed),
        (None, None) => return CandidateTimestamps::default(),
    };
    let conv = |t: crate::timestamp::NtfsTimestamp| {
        if t.is_zero() {
            None
        } else {
            Some(t.unix_seconds())
        }
    };
    let iso = |t: crate::timestamp::NtfsTimestamp| {
        if t.is_zero() {
            None
        } else {
            Some(t.to_iso8601())
        }
    };
    CandidateTimestamps {
        created: conv(c),
        modified: conv(m),
        accessed: conv(a),
        created_iso: iso(c),
        modified_iso: iso(m),
        accessed_iso: iso(a),
    }
}

/// Content of an NTFS candidate.
struct NtfsContent {
    cursor: StreamCursor,
}

impl Read for NtfsContent {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buf)
    }
}

impl CandidateContent for NtfsContent {
    fn len(&self) -> u64 {
        self.cursor.stream().len()
    }
}

impl DeletedFileProvider for NtfsUndelete {
    fn deleted_files(&self) -> Box<dyn Iterator<Item = Result<RecoveryCandidate, FsError>> + '_> {
        let resolver = self.volume.resolver();
        let iter = self
            .volume
            .files()
            .filter_map(move |(_, result)| match result {
                Ok(file) if Self::is_candidate(&file) => {
                    let path = resolver.resolve(&file);
                    Some(self.candidates_for_file(&file, &path))
                }
                _ => None,
            });
        Box::new(iter.flatten().map(Ok))
    }

    fn candidate(&self, object: &FileSystemObjectId) -> Result<RecoveryCandidate, FsError> {
        match object {
            FileSystemObjectId::Ntfs { record, stream, .. } => {
                Ok(self.candidate_for(*record, stream.as_deref())?)
            }
            other => Err(FsError::NotFound(format!("{other} is not an NTFS object"))),
        }
    }

    fn object_from_reference(&self, text: &str) -> Result<FileSystemObjectId, FsError> {
        let (record, stream) = match text.split_once(':') {
            Some((r, s)) => (r, Some(s.to_owned())),
            None => (text, None),
        };
        let record: u64 = record.trim().parse().map_err(|_| {
            FsError::NotFound(format!(
                "invalid NTFS candidate reference {text:?}; expected an MFT record number"
            ))
        })?;
        Ok(FileSystemObjectId::Ntfs {
            record,
            sequence: 0,
            stream,
        })
    }

    fn open_content(
        &self,
        candidate: &RecoveryCandidate,
    ) -> Result<Box<dyn CandidateContent>, FsError> {
        let FileSystemObjectId::Ntfs { record, stream, .. } = &candidate.filesystem_object else {
            return Err(FsError::NotFound(format!(
                "{} is not an NTFS object",
                candidate.filesystem_object
            )));
        };
        let file = self.volume.file(*record)?;
        let s = self.volume.open_stream(&file, stream.as_deref())?;
        let mut cursor = s.cursor();
        cursor
            .seek(SeekFrom::Start(0))
            .map_err(|e| FsError::Malformed {
                structure: "stream",
                detail: e.to_string(),
            })?;
        Ok(Box::new(NtfsContent { cursor }))
    }
}
