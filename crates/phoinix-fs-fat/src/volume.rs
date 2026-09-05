//! The FAT volume facade: directories, walking, reconstruction, streams.

use std::collections::HashSet;
use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_fs::{Extent, ExtentStream};
use serde::{Deserialize, Serialize};

use crate::FatError;
use crate::boot::{FatBootSector, FatVariant};
use crate::dir::{DirEntry, parse_directory};
use crate::table::{FatEntry, FatTable};

/// Largest directory read into memory.
pub const MAX_DIRECTORY_BYTES: u64 = 64 * 1024 * 1024;
/// Deepest directory nesting walked.
pub const MAX_DEPTH: usize = 128;

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
}

impl Reconstruction {
    /// Whether the layout was inferred heuristically.
    #[must_use]
    pub fn is_heuristic(&self) -> bool {
        !self.chain_known && !self.skipped_allocated.is_empty()
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
    /// other files are skipped (heuristic reconstruction).
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidChain`] for an invalid first cluster.
    pub fn reconstruct(&self, entry: &DirEntry) -> Result<Reconstruction, FatError> {
        let cs = u64::from(self.boot.cluster_size);
        let needed = u64::from(entry.size).div_ceil(cs);
        let empty = || Reconstruction {
            clusters: Vec::new(),
            chain_known: true,
            skipped_allocated: Vec::new(),
            complete: true,
            extent_count: 0,
            contiguous_span: Vec::new(),
        };
        if needed == 0 {
            return Ok(empty());
        }
        let first = self.effective_first_cluster(entry);
        if !self.boot.is_valid_cluster(first) {
            return Err(FatError::InvalidChain(format!(
                "first cluster {first} is outside the volume"
            )));
        }
        // An intact chain of the right length (allocated file, or a driver
        // that did not clear the FAT on deletion).
        if let Ok(chain) = self.fat.chain(first)
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
            });
        }
        // Contiguous assumption with skipping of allocated clusters.
        let mut clusters = Vec::new();
        let mut skipped = Vec::new();
        let mut span = Vec::new();
        let mut c = first;
        let limit = self.boot.cluster_count.saturating_add(2);
        while (clusters.len() as u64) < needed && c < limit {
            if (span.len() as u64) < needed {
                span.push(c);
            }
            match self.fat.entry(c) {
                FatEntry::Free => clusters.push(c),
                _ => skipped.push(c),
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
            skipped_allocated: skipped,
            complete,
            extent_count,
            contiguous_span: span,
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
