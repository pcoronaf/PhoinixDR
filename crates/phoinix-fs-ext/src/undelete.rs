//! ext2/3/4 deleted-file detection, journal-assisted layout recovery and
//! evidence.
//!
//! Three sources are joined per inode number:
//!
//! * the **inode table**: a deleted inode keeps its timestamps and (on old
//!   ext2 drivers) its block map, but modern kernels clear the size and
//!   the map on deletion;
//! * the **journal**: jbd2 logs whole inode-table blocks, so an older copy
//!   of the inode taken while the file was still alive often survives
//!   with its size and extent tree intact;
//! * **directory slack**: deleting an entry folds it into its predecessor,
//!   leaving the name and inode number readable.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::{Arc, OnceLock};

use phoinix_block::BlockReaderExt;
use phoinix_core::fmt::iso8601_utc;
use phoinix_core::{CandidateId, FileSystemType, SourceId};
use phoinix_fs::{
    AllocationSummary, AllocationView, ByteRange, CandidateContent, CandidateTimestamps,
    DeletedFileProvider, Extent, ExtentStreamCursor, FileSystemObjectId, FsError,
    RecoveryCandidate,
};
use phoinix_health::validate::{
    DEFAULT_BYTE_BUDGET, assess_zero_content, examine, expected_type_from_name,
};
use phoinix_health::{
    AllocationEvidence, CandidateSource, ContentEvidence, ExtentEvidence, MetadataEvidence,
    RecoveryDiagnostic, RecoveryEvidence, ScoringModel, StorageEvidence, score,
};

use crate::ExtError;
use crate::bitmap::BlockState;
use crate::inode::{Inode, InodeKind};
use crate::volume::{ExtVolume, Layout, LayoutSource, WalkedEntry};

/// Bytes of inode table read per request while scanning.
const SCAN_CHUNK: usize = 1024 * 1024;

/// Where the version of the inode used for a candidate came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InodeVersion {
    /// The inode as it is on disk now.
    Current,
    /// An older copy from the journal.
    Journal,
}

/// A deleted object assembled from the inode table, the journal and
/// directory slack.
#[derive(Debug, Clone)]
pub struct DeletedInode {
    /// Inode number.
    pub number: u32,
    /// The inode as it is on disk now, if readable.
    pub current: Option<Inode>,
    /// The layout, from the inode itself or from the journal.
    pub layout: Option<Layout>,
    /// Directory entries (live or slack) naming the inode.
    pub names: Vec<WalkedEntry>,
    /// The on-disk inode is alive again: it was reused by another file
    /// and only a journal copy describes the deleted one.
    pub reused: bool,
    /// The journal shows the file was empty when alive.
    pub known_empty: bool,
}

impl DeletedInode {
    /// The inode version the candidate describes.
    #[must_use]
    pub fn version(&self) -> Option<&Inode> {
        self.layout
            .as_ref()
            .map(|l| &l.inode)
            .or(self.current.as_ref())
    }

    /// Where [`version`](Self::version) came from.
    #[must_use]
    pub fn version_source(&self) -> InodeVersion {
        match self.layout.as_ref().map(|l| &l.source) {
            Some(LayoutSource::Journal { .. }) => InodeVersion::Journal,
            _ => InodeVersion::Current,
        }
    }

    /// Generation of the version used (identity of the candidate).
    #[must_use]
    pub fn generation(&self) -> u32 {
        self.version().map_or(0, |i| i.generation)
    }

    /// The name most consistent with the inode version used: inode numbers
    /// are reused, so a slack entry may belong to an earlier or later file.
    /// When the layout comes from journal transaction `s`, the entry that
    /// was live in `s` wins; entries whose lifetime is unknown come next,
    /// then older names.
    #[must_use]
    pub fn name(&self) -> Option<&WalkedEntry> {
        let sequence = match self.layout.as_ref().map(|l| &l.source) {
            Some(LayoutSource::Journal { sequence, .. }) => Some(*sequence),
            _ => None,
        };
        let rank = |w: &WalkedEntry| -> (u8, u32) {
            match (sequence, w.entry.alive_in) {
                (Some(s), Some((a, b))) if a <= s && s <= b => (3, b),
                (_, None) => (2, 0),
                (None, Some((_, b))) => (2, b),
                (Some(s), Some((_, b))) if b < s => (1, b),
                (Some(_), Some((_, b))) => (0, b),
            }
        };
        // Reversed so that the first entry wins ties.
        self.names.iter().rev().max_by_key(|w| rank(w))
    }
}

/// Names found in directory slack, keyed by inode number.
type NameIndex = BTreeMap<u32, Vec<WalkedEntry>>;

/// The ext undelete engine.
pub struct ExtUndelete {
    volume: Arc<ExtVolume>,
    storage: StorageEvidence,
    model: ScoringModel,
    source_id: SourceId,
    examine_content: bool,
    names: OnceLock<Result<NameIndex, ExtError>>,
}

impl std::fmt::Debug for ExtUndelete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtUndelete")
            .field("volume", &self.volume)
            .finish_non_exhaustive()
    }
}

impl ExtUndelete {
    /// Creates the engine.
    #[must_use]
    pub fn new(volume: Arc<ExtVolume>, storage: StorageEvidence) -> Self {
        let source_id = volume.reader().id();
        Self {
            volume,
            storage,
            model: ScoringModel::default(),
            source_id,
            examine_content: true,
            names: OnceLock::new(),
        }
    }

    /// Disables content examination.
    #[must_use]
    pub fn without_content_examination(mut self) -> Self {
        self.examine_content = false;
        self
    }

    /// The volume.
    #[must_use]
    pub fn volume(&self) -> &ExtVolume {
        &self.volume
    }

    /// Resolves the layout of a directory inode for the tree walk: the
    /// inode itself while alive, the journal once deleted.
    fn resolve_directory(&self, inode: &Inode) -> Option<Layout> {
        if !inode.is_deleted() && inode.has_layout() {
            return self.volume.layout_of(inode).ok();
        }
        self.volume.journal_layout(inode.number)
    }

    /// Walks the tree once and indexes every entry that names a deleted
    /// object (slack entries, and live entries inside deleted directories).
    fn name_index(&self) -> Result<&NameIndex, ExtError> {
        let result = self.names.get_or_init(|| {
            let walked = self.volume.walk(&|i| self.resolve_directory(i))?;
            let mut index: NameIndex = BTreeMap::new();
            for w in walked {
                if w.entry.inode == 0 || w.entry.is_directory() {
                    continue;
                }
                if !(w.entry.deleted || w.via_deleted_directory) {
                    continue;
                }
                index.entry(w.entry.inode).or_default().push(w);
            }
            Ok(index)
        });
        match result {
            Ok(index) => Ok(index),
            Err(e) => Err(ExtError::Malformed {
                structure: "directory tree",
                detail: e.to_string(),
            }),
        }
    }

    /// Scans the inode tables for deleted regular files.
    fn scan_deleted_inodes(&self) -> Result<BTreeSet<u32>, ExtError> {
        let sb = self.volume.superblock();
        let inode_size = usize::from(sb.inode_size);
        let per_group = sb.inodes_per_group;
        let mut out = BTreeSet::new();
        if inode_size == 0 || per_group == 0 {
            return Ok(out);
        }
        let per_chunk = (SCAN_CHUNK / inode_size).max(1);
        for (g, desc) in self.volume.groups().iter().enumerate() {
            if desc.inode_uninit() {
                continue;
            }
            let group = u32::try_from(g).map_err(|_| ExtError::Overflow)?;
            let first = group
                .checked_mul(per_group)
                .and_then(|n| n.checked_add(1))
                .ok_or(ExtError::Overflow)?;
            // Never-used tail of the table (when the driver tracks it).
            let used = per_group.saturating_sub(desc.itable_unused.min(per_group));
            let table = sb.block_offset(desc.inode_table)?;
            let mut index = 0u32;
            while index < used {
                let count = per_chunk.min(usize::try_from(used - index).unwrap_or(usize::MAX));
                let offset = table
                    .checked_add(u64::from(index) * inode_size as u64)
                    .ok_or(ExtError::Overflow)?;
                let raw = match self
                    .volume
                    .reader()
                    .read_vec(offset, count.saturating_mul(inode_size))
                {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::debug!(group, error = %e, "inode table unreadable");
                        break;
                    }
                };
                for (i, bytes) in raw.chunks_exact(inode_size).enumerate() {
                    let number = first
                        .checked_add(index)
                        .and_then(|n| n.checked_add(u32::try_from(i).ok()?))
                        .ok_or(ExtError::Overflow)?;
                    if number > sb.inodes_count {
                        break;
                    }
                    let Some(inode) = Inode::parse(number, bytes, sb) else {
                        continue;
                    };
                    if inode.is_deleted()
                        && matches!(inode.kind(), InodeKind::Regular | InodeKind::Unused)
                        && number >= sb.first_ino.max(1)
                    {
                        out.insert(number);
                    }
                }
                index = index.saturating_add(u32::try_from(count).unwrap_or(u32::MAX));
            }
        }
        Ok(out)
    }

    /// Joins the sources for inode `number`, returning `None` when nothing
    /// deleted can be described (a live inode with no earlier journal
    /// copy, a directory, a special file).
    fn assemble(&self, number: u32, names: &NameIndex) -> Option<DeletedInode> {
        let current = self.volume.inode(number).ok()?;
        let entries = names.get(&number).cloned().unwrap_or_default();
        let named = !entries.is_empty();
        let mut reused = false;
        let mut known_empty = false;
        let layout;
        if current.is_deleted() {
            if current.has_layout() {
                layout = self.volume.layout_of(&current).ok();
            } else {
                let generation = current.generation;
                layout = self
                    .volume
                    .journal_layout_where(number, &|old| old.generation == generation);
                if layout.is_none() {
                    known_empty = self
                        .volume
                        .journal_versions(number)
                        .iter()
                        .find(|(_, old)| {
                            old.generation == generation
                                && old.dtime == 0
                                && old.links > 0
                                && old.kind() == InodeKind::Regular
                        })
                        .is_some_and(|(_, old)| old.size == 0 && !old.has_layout());
                }
            }
        } else if current.mode == 0 {
            // Cleared (or never used) inode that a slack entry names.
            if !named {
                return None;
            }
            layout = self.volume.journal_layout(number);
        } else {
            // Alive: the slack name belongs to an earlier incarnation only
            // if the journal holds a copy with a different generation.
            if !named {
                return None;
            }
            let generation = current.generation;
            layout = self
                .volume
                .journal_layout_where(number, &|old| old.generation != generation);
            layout.as_ref()?;
            reused = true;
        }
        let deleted = DeletedInode {
            number,
            current: Some(current),
            layout,
            names: entries,
            reused,
            known_empty,
        };
        let kind = deleted.version().map(Inode::kind);
        match kind {
            Some(InodeKind::Regular) => Some(deleted),
            Some(InodeKind::Unused) if named => Some(deleted),
            _ => None,
        }
    }

    /// Every deleted object, by inode number.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError`] if the tree or the inode tables cannot be read.
    pub fn deleted_inodes(&self) -> Result<Vec<DeletedInode>, ExtError> {
        let names = self.name_index()?;
        let mut numbers = self.scan_deleted_inodes()?;
        numbers.extend(names.keys().copied());
        Ok(numbers
            .into_iter()
            .filter_map(|n| self.assemble(n, names))
            .collect())
    }

    /// The deleted object with inode `number`.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError::NotFound`] if the inode is not a deleted object.
    pub fn deleted_inode(&self, number: u32) -> Result<DeletedInode, ExtError> {
        let names = self.name_index()?;
        self.assemble(number, names)
            .ok_or_else(|| ExtError::NotFound(format!("inode {number} is not a deleted file")))
    }

    fn allocation_of(&self, layout: &Layout) -> AllocationEvidence {
        let mut a = AllocationEvidence {
            map_available: true,
            ..Default::default()
        };
        for run in &layout.runs {
            let Some(p) = run.physical else {
                continue;
            };
            for i in 0..run.count {
                match self.volume.block_state(p.saturating_add(i)) {
                    BlockState::Free => a.clusters_free += 1,
                    BlockState::Allocated => a.clusters_allocated += 1,
                    BlockState::Unknown => a.clusters_unknown += 1,
                }
                a.clusters_total += 1;
            }
        }
        a
    }

    /// Builds the candidate for a deleted object.
    #[must_use]
    pub fn build_candidate(&self, d: &DeletedInode) -> RecoveryCandidate {
        let bs = u64::from(self.volume.block_size().max(1));
        let mut diagnostics = Vec::new();
        let version = d.version().cloned();
        let name = d.name();
        if let Some(w) = name {
            if w.via_deleted_directory {
                diagnostics.push(RecoveryDiagnostic::info(
                    "The original path passes through a deleted directory whose entries were recovered from the journal",
                ));
            }
            if d.names.len() > 1 {
                let others: Vec<&str> = d
                    .names
                    .iter()
                    .filter(|o| o.entry.offset != w.entry.offset)
                    .map(|o| o.path.as_str())
                    .collect();
                diagnostics.push(RecoveryDiagnostic::info(format!(
                    "Other names referred to this inode: {}",
                    others.join(", ")
                )));
            }
        }
        if let Some(cur) = &d.current
            && cur.dtime != 0
        {
            diagnostics.push(RecoveryDiagnostic::info(format!(
                "Deleted at {}",
                iso8601_utc(i64::from(cur.dtime), 0)
            )));
        }
        if d.reused {
            diagnostics.push(RecoveryDiagnostic::warning(
                "The inode has since been reused by another file; the layout comes from a journal copy of the earlier file",
            ));
        }

        let mut stale = false;
        let mut valid_record = version
            .as_ref()
            .is_some_and(|v| v.checksum_ok != Some(false));
        let (extents, allocation) = match &d.layout {
            Some(layout) => {
                match &layout.source {
                    LayoutSource::Journal {
                        sequence,
                        checksum_ok,
                    } => {
                        let status = match checksum_ok {
                            Some(true) => "checksum verified",
                            Some(false) => "checksum MISMATCH",
                            None => "no checksum",
                        };
                        let d1 = format!(
                            "Layout recovered from journal transaction {sequence} ({status})"
                        );
                        if *checksum_ok == Some(false) {
                            valid_record = false;
                            diagnostics.push(RecoveryDiagnostic::warning(d1));
                        } else {
                            diagnostics.push(RecoveryDiagnostic::info(d1));
                        }
                        // Deletion itself rewrites mtime (the truncate that
                        // frees the blocks), so only a modification time
                        // that is neither the journal copy's nor the
                        // deletion time proves a later change.
                        if let Some(cur) = &d.current
                            && !d.reused
                            && cur.dtime != 0
                            && cur.mtime != 0
                            && cur.mtime != cur.dtime
                            && cur.mtime != layout.inode.mtime
                        {
                            stale = true;
                            diagnostics.push(RecoveryDiagnostic::warning(format!(
                                "The file was modified at {} after the journal copy ({}) was written",
                                iso8601_utc(i64::from(cur.mtime), 0),
                                iso8601_utc(i64::from(layout.inode.mtime), 0)
                            )));
                        }
                    }
                    LayoutSource::Inode => {
                        diagnostics.push(RecoveryDiagnostic::info(
                            "The deleted inode still carries its block map",
                        ));
                    }
                    LayoutSource::Inline => {}
                }
                let resident = layout.inline.is_some();
                let complete = self.volume.layout_complete(layout);
                let physical: Vec<_> = layout
                    .runs
                    .iter()
                    .filter(|r| r.physical.is_some())
                    .collect();
                let total: u64 = physical.iter().map(|r| r.count).sum();
                let sparse = layout
                    .runs
                    .iter()
                    .any(|r| r.physical.is_none() || r.uninitialized)
                    || (complete && !resident && total < layout.size.div_ceil(bs));
                let extents = ExtentEvidence {
                    resident,
                    complete,
                    extent_count: u32::try_from(physical.len()).unwrap_or(u32::MAX),
                    total_clusters: Some(total),
                    expected_clusters: Some(layout.size.div_ceil(bs)),
                    sparse,
                    compressed: false,
                    encrypted: false,
                    chain_known: true,
                    heuristic: false,
                    start_inferred: false,
                    stale,
                };
                let allocation = if resident {
                    AllocationEvidence {
                        map_available: true,
                        ..Default::default()
                    }
                } else {
                    self.allocation_of(layout)
                };
                (extents, allocation)
            }
            None => {
                if d.known_empty {
                    diagnostics.push(RecoveryDiagnostic::info(
                        "The journal shows the file was empty",
                    ));
                } else if self.volume.journal().is_some() {
                    diagnostics.push(RecoveryDiagnostic::warning(
                        "The block map was cleared on deletion and no journal copy of the inode survives; only carving can find the content",
                    ));
                } else {
                    diagnostics.push(RecoveryDiagnostic::warning(
                        "The filesystem has no journal and the block map was cleared on deletion; only carving can find the content",
                    ));
                }
                let expected = version
                    .as_ref()
                    .filter(|v| v.size > 0)
                    .map(|v| v.size.div_ceil(bs));
                (
                    ExtentEvidence {
                        complete: d.known_empty,
                        chain_known: true,
                        total_clusters: Some(0),
                        expected_clusters: expected,
                        ..Default::default()
                    },
                    AllocationEvidence {
                        map_available: true,
                        ..Default::default()
                    },
                )
            }
        };

        let size = match (&d.layout, &version) {
            (Some(l), _) => Some(l.size),
            (None, Some(v)) if v.size > 0 => Some(v.size),
            (None, _) if d.known_empty => Some(0),
            _ => None,
        };
        let metadata = MetadataEvidence {
            valid_record,
            filename_available: name.is_some(),
            original_parent_available: name.is_some(),
            parent_reference_valid: name.is_some() && !d.reused,
            logical_size_available: size.is_some(),
            logical_size: size,
            timestamps_available: version.as_ref().is_some_and(|v| v.mtime != 0),
        };
        let expected_type = name.and_then(|w| expected_type_from_name(&w.entry.name));
        let mut content = ContentEvidence::default();
        if self.examine_content
            && extents.complete
            && size.is_some_and(|s| s > 0)
            && let Some(layout) = &d.layout
        {
            let stream = self.volume.open_layout(layout);
            let mut cursor = stream.cursor();
            match examine(&mut cursor, stream.len(), DEFAULT_BYTE_BUDGET) {
                Ok(mut c) => {
                    c.zero_assessment = assess_zero_content(
                        c.zero_block_ratio.unwrap_or(0.0),
                        c.head_is_zero,
                        extents.sparse,
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
        content.expected_type = expected_type;
        let source = match d.version_source() {
            InodeVersion::Journal => CandidateSource::Journal,
            InodeVersion::Current => CandidateSource::FilesystemMetadata,
        };
        let evidence = RecoveryEvidence {
            source,
            metadata,
            extents,
            allocation,
            content,
            storage: self.storage.clone(),
            diagnostics,
        };
        let health = score(&evidence, &self.model);
        let ts = |t: u32| (t != 0).then_some(i64::from(t));
        let created = version.as_ref().and_then(|v| v.crtime).and_then(ts);
        let modified = version.as_ref().and_then(|v| ts(v.mtime));
        let accessed = version.as_ref().and_then(|v| ts(v.atime));
        let iso = |t: Option<i64>| t.map(|s| iso8601_utc(s, 0));
        RecoveryCandidate {
            id: CandidateId::new(),
            source_id: self.source_id,
            filesystem: FileSystemType::Ext,
            filesystem_object: FileSystemObjectId::Ext {
                inode: d.number,
                generation: d.generation(),
            },
            original_name: name.map(|w| w.entry.name.clone()),
            original_path: name.map(|w| w.path.clone()),
            path_uncertain: d.reused,
            logical_size: size,
            deleted: true,
            timestamps: CandidateTimestamps {
                created,
                modified,
                accessed,
                created_iso: iso(created),
                modified_iso: iso(modified),
                accessed_iso: iso(accessed),
            },
            evidence,
            health,
        }
    }

    fn find(&self, object: &FileSystemObjectId) -> Result<DeletedInode, FsError> {
        let FileSystemObjectId::Ext { inode, generation } = object else {
            return Err(FsError::NotFound(format!("{object} is not an ext object")));
        };
        let d = self.deleted_inode(*inode)?;
        if *generation != 0 && d.generation() != *generation {
            return Err(FsError::NotFound(format!(
                "inode {inode} now has generation {}, not {generation}",
                d.generation()
            )));
        }
        Ok(d)
    }

    /// Free state of the block at 0-based index `index` past the first
    /// data block.
    fn block_free(&self, index: u64) -> Option<bool> {
        let block = index.checked_add(u64::from(self.volume.superblock().first_data_block))?;
        match self.volume.block_state(block) {
            BlockState::Free => Some(true),
            BlockState::Allocated => Some(false),
            BlockState::Unknown => None,
        }
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

impl DeletedFileProvider for ExtUndelete {
    fn deleted_files(&self) -> Box<dyn Iterator<Item = Result<RecoveryCandidate, FsError>> + '_> {
        match self.deleted_inodes() {
            Ok(items) => Box::new(items.into_iter().map(move |d| Ok(self.build_candidate(&d)))),
            Err(e) => Box::new(std::iter::once(Err(e.into()))),
        }
    }

    fn candidate(&self, object: &FileSystemObjectId) -> Result<RecoveryCandidate, FsError> {
        Ok(self.build_candidate(&self.find(object)?))
    }

    fn object_from_reference(&self, text: &str) -> Result<FileSystemObjectId, FsError> {
        let inode: u32 = text.trim().parse().map_err(|_| {
            FsError::NotFound(format!(
                "invalid ext candidate reference {text:?}; expected an inode number"
            ))
        })?;
        Ok(FileSystemObjectId::Ext {
            inode,
            generation: 0,
        })
    }

    fn open_content(
        &self,
        candidate: &RecoveryCandidate,
    ) -> Result<Box<dyn CandidateContent>, FsError> {
        let d = self.find(&candidate.filesystem_object)?;
        let stream = match &d.layout {
            Some(layout) => self.volume.open_layout(layout),
            None if d.known_empty => self.volume.open_layout(&Layout {
                runs: Vec::new(),
                size: 0,
                source: LayoutSource::Inline,
                inline: Some(Vec::new()),
                inode: d
                    .current
                    .clone()
                    .ok_or_else(|| FsError::NotFound(format!("inode {} unreadable", d.number)))?,
            }),
            None => {
                return Err(FsError::NotFound(format!(
                    "no layout is known for inode {}",
                    d.number
                )));
            }
        };
        Ok(Box::new(Content {
            cursor: stream.cursor(),
        }))
    }

    fn content_extents(&self, candidate: &RecoveryCandidate) -> Result<Vec<Extent>, FsError> {
        let d = self.find(&candidate.filesystem_object)?;
        Ok(d.layout
            .as_ref()
            .filter(|l| l.inline.is_none())
            .map(|l| self.volume.extents_of(l))
            .unwrap_or_default())
    }
}

impl AllocationView for ExtUndelete {
    fn cluster_size(&self) -> u64 {
        u64::from(self.volume.block_size().max(1))
    }

    fn volume_len(&self) -> u64 {
        self.volume
            .superblock()
            .blocks_count
            .saturating_mul(AllocationView::cluster_size(self))
    }

    fn map_available(&self) -> bool {
        true
    }

    fn free_ranges(&self) -> Result<Vec<ByteRange>, FsError> {
        let sb = self.volume.superblock();
        let first = u64::from(sb.first_data_block);
        Ok(phoinix_fs::space::free_ranges_from(
            sb.blocks_count.saturating_sub(first),
            AllocationView::cluster_size(self),
            first.saturating_mul(AllocationView::cluster_size(self)),
            |b| self.block_free(b),
        ))
    }

    fn summarize(&self, range: ByteRange) -> AllocationSummary {
        let sb = self.volume.superblock();
        let first = u64::from(sb.first_data_block);
        phoinix_fs::space::summarize_with(
            range,
            AllocationView::cluster_size(self),
            first.saturating_mul(AllocationView::cluster_size(self)),
            sb.blocks_count.saturating_sub(first),
            |b| self.block_free(b),
        )
    }
}
