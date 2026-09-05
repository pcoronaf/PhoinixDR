//! The allocation bitmap: one bit per cluster, 1 = allocated.

use phoinix_block::{BlockReader, BlockReaderExt};
use serde::{Deserialize, Serialize};

use crate::ExfatError;
use crate::boot::ExfatBootSector;
use crate::table::ExfatTable;

/// Largest bitmap loaded.
pub const MAX_BITMAP_BYTES: u64 = 256 * 1024 * 1024;

/// Allocation state of a cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterState {
    /// Free.
    Free,
    /// Allocated.
    Allocated,
    /// Outside the bitmap.
    Unknown,
}

/// Summary of a cluster range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RangeAllocation {
    /// Free clusters.
    pub free: u64,
    /// Allocated clusters.
    pub allocated: u64,
    /// Unknown clusters.
    pub unknown: u64,
}

/// The bitmap in memory.
#[derive(Debug, Clone)]
pub struct AllocationBitmap {
    bits: Vec<u8>,
    cluster_count: u32,
}

impl AllocationBitmap {
    /// Loads the bitmap whose directory entry names `first_cluster` and
    /// `length` bytes; the bitmap's own chain is followed through the FAT
    /// and assumed contiguous when the FAT does not describe it.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError`] on read failures.
    pub fn load(
        reader: &dyn BlockReader,
        boot: &ExfatBootSector,
        fat: &ExfatTable,
        first_cluster: u32,
        length: u64,
    ) -> Result<Self, ExfatError> {
        if length > MAX_BITMAP_BYTES {
            return Err(ExfatError::Unsupported(
                "allocation bitmap too large".into(),
            ));
        }
        let cs = u64::from(boot.cluster_size);
        let clusters_needed = length.div_ceil(cs);
        let chain = match fat.chain(first_cluster, clusters_needed) {
            Ok(c) if c.len() as u64 == clusters_needed => c,
            _ => (0..clusters_needed)
                .map(|i| first_cluster.saturating_add(u32::try_from(i).unwrap_or(u32::MAX)))
                .collect(),
        };
        let mut bits =
            Vec::with_capacity(usize::try_from(length).map_err(|_| ExfatError::Overflow)?);
        let mut remaining = length;
        for c in chain {
            let take = usize::try_from(remaining.min(cs)).map_err(|_| ExfatError::Overflow)?;
            if take == 0 {
                break;
            }
            let mut buf = vec![0u8; take];
            reader.read_exact_at(boot.cluster_offset(c)?, &mut buf)?;
            bits.extend_from_slice(&buf);
            remaining -= take as u64;
        }
        Ok(Self {
            bits,
            cluster_count: boot.cluster_count,
        })
    }

    /// Wraps raw bytes.
    #[must_use]
    pub fn from_bytes(bits: Vec<u8>, cluster_count: u32) -> Self {
        Self {
            bits,
            cluster_count,
        }
    }

    /// State of `cluster`.
    #[must_use]
    pub fn state(&self, cluster: u32) -> ClusterState {
        if cluster < 2 || cluster - 2 >= self.cluster_count {
            return ClusterState::Unknown;
        }
        let index = cluster - 2;
        match self
            .bits
            .get(usize::try_from(index / 8).unwrap_or(usize::MAX))
        {
            None => ClusterState::Unknown,
            Some(b) => {
                if b & (1u8 << (index % 8)) != 0 {
                    ClusterState::Allocated
                } else {
                    ClusterState::Free
                }
            }
        }
    }

    /// Summarises `clusters`.
    #[must_use]
    pub fn summarize(&self, clusters: impl IntoIterator<Item = u32>) -> RangeAllocation {
        let mut out = RangeAllocation::default();
        for c in clusters {
            match self.state(c) {
                ClusterState::Free => out.free += 1,
                ClusterState::Allocated => out.allocated += 1,
                ClusterState::Unknown => out.unknown += 1,
            }
        }
        out
    }

    /// Number of allocated clusters.
    #[must_use]
    pub fn allocated_clusters(&self) -> u64 {
        (2..self.cluster_count.saturating_add(2))
            .filter(|c| self.state(*c) == ClusterState::Allocated)
            .count() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bits() {
        let bm = AllocationBitmap::from_bytes(vec![0b0000_0101], 8);
        assert_eq!(bm.state(2), ClusterState::Allocated);
        assert_eq!(bm.state(3), ClusterState::Free);
        assert_eq!(bm.state(4), ClusterState::Allocated);
        assert_eq!(bm.state(20), ClusterState::Unknown);
        assert_eq!(
            bm.summarize([2, 3, 4, 20]),
            RangeAllocation {
                free: 1,
                allocated: 2,
                unknown: 1
            }
        );
        assert_eq!(bm.allocated_clusters(), 2);
    }
}
