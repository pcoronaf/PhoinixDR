//! The FAT volume facade: directories, walking, reconstruction, streams.

use std::collections::HashSet;
use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_fs::{Extent, ExtentStream};
use phoinix_health::validate::{SIGNATURES, detect_type, expected_type_from_name};
use serde::{Deserialize, Serialize};

use crate::FatError;
use crate::boot::{FatBootSector, FatVariant};
use crate::dir::{DirEntry, parse_directory};
use crate::table::{FatEntry, FatTable};

/// Largest directory read into memory.
pub const MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;
/// Deepest directory nesting walked.
pub const MAX_DEPTH: usize = 128;
/// Allocated clusters skipped before a contiguous reconstruction gives up.
pub const MAX_SKIPPED_CLUSTERS: usize = 65_536;
/// Bytes read from each candidate start cluster when inferring the start.
pub const START_PROBE_BYTES: usize = 4096;

/// Strength of the content evidence behind an inferred start cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartEvidence {
    /// The content at the chosen cluster carries the signature of the type
    /// expected from the file name.
    ExpectedType,
    /// The content starts with a recognisable file signature (no type was
    /// expected from the name).
    KnownType,
    /// The content is merely not zero-filled: weak evidence.
    NonZero,
}

impl StartEvidence {
    /// Wording for diagnostics.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            StartEvidence::ExpectedType => {
                "its content carries the signature of the type expected from the file name"
            }
            StartEvidence::KnownType => "its content starts with a recognisable file signature",
            StartEvidence::NonZero => {
                "it is the highest free candidate holding non-zero data (weak evidence)"
            }
        }
    }
}

/// A start cluster inferred because the recorded one could not be trusted.
///
/// Windows clears the high 16 bits of the first cluster when it deletes a
/// file on FAT32. On volumes with more than 65 536 clusters the surviving
/// low word then points to the wrong place. Every cluster sharing that low
/// word is a candidate; free ones are probed and ranked by their content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferredStart {
    /// The first cluster recorded in the directory entry.
    pub recorded: u32,
    /// Whether the recorded cluster is currently allocated to other data.
    pub recorded_allocated: bool,
    /// The cluster used instead.
    pub chosen: u32,
    /// Free clusters sharing the recorded low word that were probed.
    pub candidates: u32,
    /// Why the chosen cluster was preferred.
    pub evidence: StartEvidence,
}

/// How a file's clusters were determined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reconstruction {
    /// Clusters in logical order.
    pub clusters: Vec<u32>,
    /// The chain came from the FAT (allocated file, or a deleted file whose
    /// chain survived).
    pub chain_known: bool,
    /// Clusters that were skipped because they are allocated to other files.
    pub skipped_allocated: Vec<u32>,
    /// Whether every cluster of the declared size was located.
    pub complete: bool,
    /// Number of contiguous extents.
    pub extent_count: u32,
    /// Clusters of the plain contiguous span (before skipping), used for
    /// allocation evidence.
    pub contiguous_span: Vec<u32>,
    /// The start cluster was inferred rather than taken from the entry.
    pub inferred_start: Option<InferredStart>,
    /// The contiguous search gave up: [`MAX_SKIPPED_CLUSTERS`] allocated
    /// clusters follow the start, so the start is probably wrong.
    pub search_exhausted: bool,
}

impl Reconstruction {
    /// Whether the layout was inferred heuristically by skipping clusters.
    #[must_use]
    pub fn is_heuristic(&self) -> bool {
        !self.chain_known && !self.skipped_allocated.is_empty()
    }

    /// The first cluster actually used, if any.
    #[must_use]
    pub fn first_cluster(&self) -> Option<u32> {
        self.clusters.first().copied()
    }
}

/// One entry found while walking the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalkedEntry {
    /// Windows-style path (`\docs\photo.jpg`).
    pub path: String,
    /// The entry.
    pub entry: DirEntry,
    /// Whether an ancestor directory is deleted.
    pub via_deleted_directory: bool,
}

/// An opened FAT volume.
pub struct FatVolume {
    reader: Arc<dyn BlockReader>,
    boot: FatBootSector,
    fat: FatTable,
}

impl std::fmt::Debug for FatVolume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FatVolume")
            .field("source", &self.reader.describe())
            .field("boot", &self.boot)
            .finish()
    }
}

impl FatVolume {
    /// Opens the volume at offset 0 of `reader`.
    ///
    /// # Errors
    ///
    /// Returns [`FatError`] if the boot sector or FAT cannot be read.
    pub fn open(reader: Arc<dyn BlockReader>) -> Result<Self, FatError> {
        let sector = reader.read_vec(0, 512)?;
        let boot = FatBootSector::parse(&sector)?;
        let fat = FatTable::load(&*reader, &boot)?;
        tracing::info!(variant = %boot.variant, clusters = boot.cluster_count, cluster_size = boot.cluster_size, "FAT volume opened");
        Ok(Self { reader, boot, fat })
    }

    /// The boot sector.
    #[must_use]
    pub const fn boot(&self) -> &FatBootSector {
        &self.boot
    }

    /// The FAT.
    #[must_use]
    pub const fn fat(&self) -> &FatTable {
        &self.fat
    }

    /// The reader.
    #[must_use]
    pub fn reader(&self) -> &Arc<dyn BlockReader> {
        &self.reader
    }

    /// Variant.
    #[must_use]
    pub const fn variant(&self) -> FatVariant {
        self.boot.variant
    }

    /// Cluster size in bytes.
    #[must_use]
    pub const fn cluster_size(&self) -> u32 {
        self.boot.cluster_size
    }

    /// Reads the root directory.
    ///
    /// # Errors
    ///
    /// Returns [`FatError`] on read failures.
    pub fn root_directory(&self) -> Result<Vec<DirEntry>, FatError> {
        match self.boot.variant {
            FatVariant::Fat32 => self.read_directory(self.boot.root_cluster, false),
            _ => {
                let len = usize::try_from(self.boot.root_dir_bytes.min(MAX_DIRECTORY_BYTES))
                    .map_err(|_| FatError::Overflow)?;
                let bytes = self.reader.read_vec(self.boot.root_dir_offset, len)?;
                Ok(parse_directory(&bytes, self.boot.root_dir_offset))
            }
        }
    }

    /// Reads a directory starting at `first`. For a deleted directory
    /// (`deleted`) only its first cluster is read, because the chain is
    /// gone.
    ///
    /// # Errors
    ///
    /// Returns [`FatError`] on read failures or an invalid first cluster.
    pub fn read_directory(&self, first: u32, deleted: bool) -> Result<Vec<DirEntry>, FatError> {
        let clusters = if deleted {
            vec![first]
        } else {
            match self.fat.chain(first) {
                Ok(c) => c,
                Err(_) => vec![first],
            }
        };
        let cs = usize::try_from(self.boot.cluster_size).map_err(|_| FatError::Overflow)?;
        let mut out = Vec::new();
        let mut buf = vec![0u8; cs];
        let max = usize::try_from(MAX_DIRECTORY_BYTES / u64::from(self.boot.cluster_size))
            .unwrap_or(1024);
        for c in clusters.iter().take(max) {
            let off = self.boot.cluster_offset(*c)?;
            self.reader.read_exact_at(off, &mut buf)?;
            let entries = parse_directory(&buf, off);
            let ended = buf.chunks_exact(32).any(|e| e.first() == Some(&0));
            out.extend(entries);
            if ended {
                break;
            }
        }
        Ok(out)
    }

    /// Walks the tree, including deleted entries and the first cluster of
    /// deleted directories.
    ///
    /// # Errors
    ///
    /// Returns [`FatError`] if the root directory cannot be read.
    pub fn walk(&self) -> Result<Vec<WalkedEntry>, FatError> {
        let mut out = Vec::new();
        let mut visited: HashSet<u32> = HashSet::new();
        let mut stack: Vec<(String, Vec<DirEntry>, bool, usize)> =
            vec![(String::new(), self.root_directory()?, false, 0)];
        while let Some((prefix, entries, via_deleted, depth)) = stack.pop() {
            for entry in entries {
                if entry.is_dot() || entry.attributes.is_volume_label() {
                    continue;
                }
                let path = format!("{prefix}\\{}", entry.name());
                let deleted_here = via_deleted || entry.deleted;
                if entry.attributes.is_directory()
                    && depth < MAX_DEPTH
                    && self.boot.is_valid_cluster(entry.first_cluster)
                    && visited.insert(entry.first_cluster)
                {
                    match self.read_directory(entry.first_cluster, entry.deleted) {
                        Ok(children) => {
                            stack.push((path.clone(), children, deleted_here, depth + 1))
                        }
                        Err(e) => tracing::debug!(path = %path, error = %e, "directory unreadable"),
                    }
                }
                out.push(WalkedEntry {
                    path,
                    entry,
                    via_deleted_directory: via_deleted,
                });
            }
        }
        Ok(out)
    }

    /// Determines the clusters of a file.
    ///
    /// Allocated files follow the FAT chain. Deleted files whose chain is
    /// gone are assumed contiguous; clusters in that span now allocated to
    /// other files are skipped (heuristic reconstruction). On large FAT32
    /// volumes a deleted entry whose first-cluster high word is zero has
    /// its start inferred, see [`InferredStart`].
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidChain`] for an invalid first cluster.
    pub fn reconstruct(&self, entry: &DirEntry) -> Result<Reconstruction, FatError> {
        let cs = u64::from(self.boot.cluster_size);
        let needed = u64::from(entry.size).div_ceil(cs);
        if needed == 0 {
            return Ok(Reconstruction {
                clusters: Vec::new(),
                chain_known: true,
                skipped_allocated: Vec::new(),
                complete: true,
                extent_count: 0,
                contiguous_span: Vec::new(),
                inferred_start: None,
                search_exhausted: false,
            });
        }
        let recorded = self.effective_first_cluster(entry);
        let inferred = if entry.deleted && self.high_word_untrustworthy(entry) {
            self.infer_start(entry, recorded, needed)
        } else {
            None
        };
        let first = inferred.as_ref().map_or(recorded, |i| i.chosen);
        if !self.boot.is_valid_cluster(first) {
            return Err(FatError::InvalidChain(format!(
                "first cluster {first} is outside the volume"
            )));
        }
        // An intact chain of the right length (allocated file, or a driver
        // that did not clear the FAT on deletion).
        if inferred.is_none()
            && let Ok(chain) = self.fat.chain(first)
            && chain.len() as u64 >= needed
            && (!entry.deleted || chain.len() as u64 == needed)
        {
            let clusters: Vec<u32> = chain
                .into_iter()
                .take(usize::try_from(needed).unwrap_or(usize::MAX))
                .collect();
            let extent_count = count_extents(&clusters);
            let span = clusters.clone();
            return Ok(Reconstruction {
                clusters,
                chain_known: true,
                skipped_allocated: Vec::new(),
                complete: true,
                extent_count,
                contiguous_span: span,
                inferred_start: None,
                search_exhausted: false,
            });
        }
        let mut r = self.contiguous_from(first, needed);
        r.inferred_start = inferred;
        Ok(r)
    }

    /// Contiguous assumption from `first`, skipping allocated clusters.
    fn contiguous_from(&self, first: u32, needed: u64) -> Reconstruction {
        let mut clusters = Vec::new();
        let mut skipped = Vec::new();
        let mut span = Vec::new();
        let mut c = first;
        let limit = self.boot.cluster_count.saturating_add(2);
        let mut exhausted = false;
        while (clusters.len() as u64) < needed && c < limit {
            if (span.len() as u64) < needed {
                span.push(c);
            }
            match self.fat.entry(c) {
                FatEntry::Free => clusters.push(c),
                _ => skipped.push(c),
            }
            c = c.saturating_add(1);
            if skipped.len() > MAX_SKIPPED_CLUSTERS {
                exhausted = true;
                break;
            }
        }
        let complete = clusters.len() as u64 == needed;
        let extent_count = count_extents(&clusters);
        Reconstruction {
            clusters,
            chain_known: false,
            skipped_allocated: skipped,
            complete,
            extent_count,
            contiguous_span: span,
            inferred_start: None,
            search_exhausted: exhausted,
        }
    }

    /// Whether the entry's first-cluster high word may have been cleared on
    /// deletion: FAT32, high word zero, and a volume large enough for the
    /// high word to matter.
    #[must_use]
    pub fn high_word_untrustworthy(&self, entry: &DirEntry) -> bool {
        self.boot.variant == FatVariant::Fat32
            && entry.first_cluster_high == 0
            && self.boot.cluster_count > 0xFFFF
    }

    /// Infers the start of a deleted file whose first-cluster high word is
    /// untrustworthy. Every cluster sharing the recorded low word is a
    /// candidate; free candidates are probed and ranked by content evidence
    /// ([`StartEvidence`]), the recorded cluster wins ties against
    /// alternatives, and among alternatives the highest cluster wins because
    /// new files land after existing data.
    ///
    /// Returns `None` when the recorded cluster remains the best choice.
    #[must_use]
    pub fn infer_start(
        &self,
        entry: &DirEntry,
        recorded: u32,
        needed: u64,
    ) -> Option<InferredStart> {
        if needed == 0 {
            return None;
        }
        let low = recorded & 0xFFFF;
        let max_high = self.boot.cluster_count.saturating_add(1) >> 16;
        let expected = expected_type_from_name(entry.name()).map(|d| d.id);
        let probe =
            usize::try_from(u64::from(self.boot.cluster_size).min(START_PROBE_BYTES as u64))
                .unwrap_or(START_PROBE_BYTES);
        let mut buf = vec![0u8; probe];
        let mut best: Option<(u8, u32)> = None;
        let mut candidates = 0u32;
        let recorded_allocated =
            self.boot.is_valid_cluster(recorded) && !self.fat.is_free(recorded);
        for high in 0..=max_high {
            let c = (high << 16) | low;
            if !self.boot.is_valid_cluster(c) || !self.fat.is_free(c) {
                continue;
            }
            candidates = candidates.saturating_add(1);
            let Ok(off) = self.boot.cluster_offset(c) else {
                continue;
            };
            if self.reader.read_exact_at(off, &mut buf).is_err() {
                continue;
            }
            let score = content_score(&buf, expected.as_deref());
            let better = match best {
                None => score > 0,
                // The recorded cluster keeps ties; later (higher) clusters
                // replace an earlier alternative on equal evidence.
                Some((bs, bc)) => score > bs || (score == bs && bc != recorded),
            };
            if better {
                best = Some((score, c));
            }
        }
        let (score, chosen) = best?;
        if chosen == recorded {
            return None;
        }
        let evidence = match score {
            3 => StartEvidence::ExpectedType,
            2 => StartEvidence::KnownType,
            _ => StartEvidence::NonZero,
        };
        tracing::debug!(
            name = entry.name(),
            recorded,
            chosen,
            candidates,
            ?evidence,
            "start cluster inferred"
        );
        Some(InferredStart {
            recorded,
            recorded_allocated,
            chosen,
            candidates,
            evidence,
        })
    }

    /// The first cluster to use, compensating for FAT32 drivers that clear
    /// the high word on deletion.
    #[must_use]
    pub fn effective_first_cluster(&self, entry: &DirEntry) -> u32 {
        if self.boot.variant == FatVariant::Fat32 {
            entry.first_cluster
        } else {
            entry.first_cluster & 0xFFFF
        }
    }

    /// Byte extents for `clusters` covering `length` bytes.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidChain`] for invalid clusters.
    pub fn extents(&self, clusters: &[u32], length: u64) -> Result<Vec<Extent>, FatError> {
        let cs = u64::from(self.boot.cluster_size);
        let mut out: Vec<Extent> = Vec::new();
        let mut remaining = length;
        for c in clusters {
            if remaining == 0 {
                break;
            }
            let off = self.boot.cluster_offset(*c)?;
            let take = remaining.min(cs);
            match out.last_mut() {
                Some(last) if last.offset + last.length == off => last.length += take,
                _ => out.push(Extent {
                    offset: off,
                    length: take,
                }),
            }
            remaining -= take;
        }
        Ok(out)
    }

    /// Opens a stream over an entry's data.
    ///
    /// # Errors
    ///
    /// Returns [`FatError`] for invalid clusters.
    pub fn open_stream(&self, entry: &DirEntry) -> Result<ExtentStream, FatError> {
        let r = self.reconstruct(entry)?;
        let extents = self.extents(&r.clusters, u64::from(entry.size))?;
        let covered: u64 = extents.iter().map(|e| e.length).sum();
        Ok(ExtentStream::new(
            self.reader.clone(),
            extents,
            covered.min(u64::from(entry.size)),
        ))
    }
}

/// Ranks the content at a candidate start: 3 = matches the type expected
/// from the name, 2 = some recognisable signature, 1 = non-zero data,
/// 0 = zero-filled or contradicting the expected type.
fn content_score(head: &[u8], expected: Option<&str>) -> u8 {
    let detected = detect_type(head).map(|s| s.id);
    // Families that share a container signature.
    fn family(id: &str) -> &str {
        match id {
            "docx" | "xlsx" | "pptx" | "odf" | "jar" => "zip",
            "doc" | "xls" | "ppt" => "ole",
            other => other,
        }
    }
    let non_zero = head.iter().any(|b| *b != 0);
    match (detected, expected.map(family)) {
        (Some(d), Some(e)) if d == e => 3,
        (Some(_), Some(_)) => 0,
        (Some(_), None) => 2,
        // Expected a type with a known signature but did not see it.
        (None, Some(e)) if SIGNATURES.iter().any(|s| s.id == e) => 0,
        (None, _) => u8::from(non_zero),
    }
}

fn count_extents(clusters: &[u32]) -> u32 {
    let mut n = 0u32;
    let mut prev: Option<u32> = None;
    for c in clusters {
        if prev.is_none_or(|p| p.saturating_add(1) != *c) {
            n = n.saturating_add(1);
        }
        prev = Some(*c);
    }
    n
}
