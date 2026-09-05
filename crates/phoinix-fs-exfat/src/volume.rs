//! The exFAT volume facade: directories, walking, extents and streams.

use std::collections::HashSet;
use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_fs::{Extent, ExtentStream};
use serde::{Deserialize, Serialize};

use crate::ExfatError;
use crate::bitmap::{AllocationBitmap, ClusterState};
use crate::boot::ExfatBootSector;
use crate::dir::{Directory, EntrySet, SpecialEntry, parse_directory};
use crate::table::ExfatTable;

/// Largest directory read into memory.
pub const MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;
/// Deepest directory nesting walked.
pub const MAX_DEPTH: usize = 128;

/// How a file's clusters were determined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reconstruction {
    /// Clusters in logical order.
    pub clusters: Vec<u32>,
    /// The layout comes from the `NoFatChain` flag or an intact FAT chain.
    pub chain_known: bool,
    /// Contiguity was assumed because the FAT chain was gone.
    pub assumed_contiguous: bool,
    /// Clusters skipped because the bitmap marks them allocated to other
    /// files (heuristic fragmented reconstruction).
    pub skipped_allocated: Vec<u32>,
    /// The plain contiguous span (before skipping), for allocation evidence.
    pub contiguous_span: Vec<u32>,
    /// Whether every cluster of the declared length was located.
    pub complete: bool,
    /// Number of contiguous extents.
    pub extent_count: u32,
}

/// One entry found while walking the tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalkedEntry {
    /// Windows-style path (`\docs\photo.jpg`).
    pub path: String,
    /// The entry set.
    pub entry: EntrySet,
    /// Whether an ancestor directory is deleted.
    pub via_deleted_directory: bool,
}

/// An opened exFAT volume.
pub struct ExfatVolume {
    reader: Arc<dyn BlockReader>,
    boot: ExfatBootSector,
    fat: ExfatTable,
    bitmap: Option<AllocationBitmap>,
    label: Option<String>,
    boot_checksum_ok: Option<bool>,
}

impl std::fmt::Debug for ExfatVolume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExfatVolume")
            .field("source", &self.reader.describe())
            .field("boot", &self.boot)
            .finish()
    }
}

impl ExfatVolume {
    /// Opens the volume at offset 0 of `reader`.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError`] if the boot sector, FAT or root directory
    /// cannot be read.
    pub fn open(reader: Arc<dyn BlockReader>) -> Result<Self, ExfatError> {
        let sector = reader.read_vec(0, 512)?;
        let boot = ExfatBootSector::parse(&sector)?;
        let boot_checksum_ok = boot.verify_region_checksum(&*reader);
        let fat = ExfatTable::load(&*reader, &boot)?;
        let mut volume = Self {
            reader,
            boot,
            fat,
            bitmap: None,
            label: None,
            boot_checksum_ok,
        };
        let root = volume.read_directory(volume.boot.root_cluster, false, None)?;
        for special in &root.specials {
            match special {
                SpecialEntry::Bitmap {
                    first_cluster,
                    length,
                } => {
                    match AllocationBitmap::load(
                        &*volume.reader,
                        &volume.boot,
                        &volume.fat,
                        *first_cluster,
                        *length,
                    ) {
                        Ok(b) => volume.bitmap = Some(b),
                        Err(e) => tracing::warn!(error = %e, "exFAT allocation bitmap unavailable"),
                    }
                }
                SpecialEntry::Label(l) => volume.label = Some(l.clone()),
                SpecialEntry::UpCase { .. } => {}
            }
        }
        tracing::info!(
            clusters = volume.boot.cluster_count,
            cluster_size = volume.boot.cluster_size,
            "exFAT volume opened"
        );
        Ok(volume)
    }

    /// The boot sector.
    #[must_use]
    pub const fn boot(&self) -> &ExfatBootSector {
        &self.boot
    }

    /// The reader.
    #[must_use]
    pub fn reader(&self) -> &Arc<dyn BlockReader> {
        &self.reader
    }

    /// The allocation bitmap, if loaded.
    #[must_use]
    pub const fn bitmap(&self) -> Option<&AllocationBitmap> {
        self.bitmap.as_ref()
    }

    /// The volume label.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }

    /// Whether the boot-region checksum verified.
    #[must_use]
    pub const fn boot_checksum_ok(&self) -> Option<bool> {
        self.boot_checksum_ok
    }

    /// Cluster size.
    #[must_use]
    pub const fn cluster_size(&self) -> u32 {
        self.boot.cluster_size
    }

    /// Clusters of a directory: FAT chain unless `no_fat_chain`, in which
    /// case `length` bytes contiguous from `first`.
    fn directory_clusters(
        &self,
        first: u32,
        no_fat_chain: bool,
        length: Option<u64>,
    ) -> Result<Vec<u32>, ExfatError> {
        let cs = u64::from(self.boot.cluster_size);
        if no_fat_chain {
            let n = length.unwrap_or(cs).div_ceil(cs).max(1);
            return Ok((0..n)
                .map(|i| first.saturating_add(u32::try_from(i).unwrap_or(u32::MAX)))
                .filter(|c| self.boot.is_valid_cluster(*c))
                .collect());
        }
        let max = MAX_DIRECTORY_BYTES / cs;
        match self.fat.chain(first, max) {
            Ok(c) => Ok(c),
            Err(_) => {
                // A deleted directory's chain is gone: read its first cluster only.
                Ok(vec![first])
            }
        }
    }

    /// Reads and parses a directory.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError`] on read failures.
    pub fn read_directory(
        &self,
        first: u32,
        no_fat_chain: bool,
        length: Option<u64>,
    ) -> Result<Directory, ExfatError> {
        let clusters = self.directory_clusters(first, no_fat_chain, length)?;
        let cs = usize::try_from(self.boot.cluster_size).map_err(|_| ExfatError::Overflow)?;
        let mut dir = Directory::default();
        let mut buf = vec![0u8; cs];
        // Parse cluster by cluster; entry sets never span clusters in
        // practice, but a set at a boundary is simply parsed on the next
        // read of the joined buffer.
        let mut joined = Vec::with_capacity(cs * clusters.len().min(64));
        let mut base = None;
        for c in clusters.iter().take(
            usize::try_from(MAX_DIRECTORY_BYTES / u64::from(self.boot.cluster_size))
                .unwrap_or(1024),
        ) {
            let off = self.boot.cluster_offset(*c)?;
            if base.is_none() {
                base = Some(off);
            }
            self.reader.read_exact_at(off, &mut buf)?;
            joined.extend_from_slice(&buf);
            // Offsets are only exact for the first cluster; entry offsets in
            // later clusters are computed per cluster below.
        }
        // Parse each cluster separately so entry offsets are exact.
        let mut parsed_any = false;
        for (i, c) in clusters.iter().enumerate() {
            let Some(chunk) = joined.get(i * cs..(i + 1) * cs) else {
                break;
            };
            let off = self.boot.cluster_offset(*c)?;
            let d = parse_directory(chunk, off);
            parsed_any = true;
            dir.entries.extend(d.entries);
            dir.specials.extend(d.specials);
            if chunk.chunks_exact(32).any(|e| e.first() == Some(&0)) {
                break;
            }
        }
        if !parsed_any {
            return Err(ExfatError::Malformed("empty directory chain".into()));
        }
        Ok(dir)
    }

    /// Walks the whole tree, including deleted entries and the first cluster
    /// of deleted directories.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError`] if the root directory cannot be read.
    pub fn walk(&self) -> Result<Vec<WalkedEntry>, ExfatError> {
        let mut out = Vec::new();
        let mut visited: HashSet<u32> = HashSet::new();
        let mut stack: Vec<(String, u32, bool, Option<u64>, bool, usize)> =
            vec![(String::new(), self.boot.root_cluster, false, None, false, 0)];
        while let Some((prefix, first, no_chain, length, via_deleted, depth)) = stack.pop() {
            if !visited.insert(first) || depth > MAX_DEPTH {
                continue;
            }
            let dir = match self.read_directory(first, no_chain, length) {
                Ok(d) => d,
                Err(e) => {
                    tracing::debug!(cluster = first, error = %e, "directory unreadable");
                    continue;
                }
            };
            for entry in dir.entries {
                let path = format!("{prefix}\\{}", entry.name);
                let deleted_here = via_deleted || entry.deleted;
                if entry.is_directory()
                    && entry.first_cluster != 0
                    && self.boot.is_valid_cluster(entry.first_cluster)
                {
                    stack.push((
                        path.clone(),
                        entry.first_cluster,
                        entry.flags.no_fat_chain(),
                        Some(entry.data_length),
                        deleted_here,
                        depth + 1,
                    ));
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
    /// # Errors
    ///
    /// Returns [`ExfatError`] only for invalid first clusters.
    pub fn reconstruct(&self, entry: &EntrySet) -> Result<Reconstruction, ExfatError> {
        let cs = u64::from(self.boot.cluster_size);
        let needed = entry.data_length.div_ceil(cs);
        if needed == 0 || entry.first_cluster == 0 {
            return Ok(Reconstruction {
                clusters: Vec::new(),
                chain_known: true,
                assumed_contiguous: false,
                skipped_allocated: Vec::new(),
                contiguous_span: Vec::new(),
                complete: true,
                extent_count: 0,
            });
        }
        if !self.boot.is_valid_cluster(entry.first_cluster) {
            return Err(ExfatError::Malformed(format!(
                "first cluster {} outside the heap",
                entry.first_cluster
            )));
        }
        let contiguous = |n: u64| -> Vec<u32> {
            (0..n)
                .map(|i| {
                    entry
                        .first_cluster
                        .saturating_add(u32::try_from(i).unwrap_or(u32::MAX))
                })
                .filter(|c| self.boot.is_valid_cluster(*c))
                .collect()
        };
        if entry.flags.no_fat_chain() {
            let clusters = contiguous(needed);
            let complete = clusters.len() as u64 == needed;
            let span = clusters.clone();
            return Ok(Reconstruction {
                clusters,
                chain_known: true,
                assumed_contiguous: false,
                skipped_allocated: Vec::new(),
                contiguous_span: span,
                complete,
                extent_count: 1,
            });
        }
        match self.fat.chain(entry.first_cluster, needed) {
            Ok(chain) if chain.len() as u64 == needed => {
                let extent_count = count_extents(&chain);
                let span = chain.clone();
                Ok(Reconstruction {
                    clusters: chain,
                    chain_known: true,
                    assumed_contiguous: false,
                    skipped_allocated: Vec::new(),
                    contiguous_span: span,
                    complete: true,
                    extent_count,
                })
            }
            _ => {
                // Chain gone: assume contiguity, skipping clusters the bitmap
                // shows as allocated to other files.
                let mut clusters = Vec::new();
                let mut skipped = Vec::new();
                let mut span = Vec::new();
                let mut c = entry.first_cluster;
                let limit = self.boot.cluster_count.saturating_add(2);
                while (clusters.len() as u64) < needed && c < limit {
                    if (span.len() as u64) < needed {
                        span.push(c);
                    }
                    match self.cluster_state(c) {
                        ClusterState::Allocated => skipped.push(c),
                        _ => clusters.push(c),
                    }
                    c = c.saturating_add(1);
                    if skipped.len() > 65_536 {
                        break;
                    }
                }
                let complete = clusters.len() as u64 == needed;
                let extent_count = count_extents(&clusters);
                Ok(Reconstruction {
                    clusters,
                    chain_known: false,
                    assumed_contiguous: true,
                    skipped_allocated: skipped,
                    contiguous_span: span,
                    complete,
                    extent_count,
                })
            }
        }
    }

    /// Builds the byte extents of a reconstruction.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError`] for invalid clusters.
    pub fn extents(&self, clusters: &[u32], length: u64) -> Result<Vec<Extent>, ExfatError> {
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
    /// Returns [`ExfatError`] for invalid clusters.
    pub fn open_stream(&self, entry: &EntrySet) -> Result<ExtentStream, ExfatError> {
        let r = self.reconstruct(entry)?;
        let extents = self.extents(&r.clusters, entry.data_length)?;
        let covered: u64 = extents.iter().map(|e| e.length).sum();
        Ok(ExtentStream::new(
            self.reader.clone(),
            extents,
            covered.min(entry.data_length),
        ))
    }

    /// Allocation state of a cluster.
    #[must_use]
    pub fn cluster_state(&self, cluster: u32) -> ClusterState {
        self.bitmap
            .as_ref()
            .map_or(ClusterState::Unknown, |b| b.state(cluster))
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
