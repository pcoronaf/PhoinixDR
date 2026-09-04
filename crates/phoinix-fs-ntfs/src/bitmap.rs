//! `$Bitmap`: one bit per cluster, 1 = allocated.

use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::mft::BITMAP_RECORD;
use crate::volume::NtfsVolume;

/// Largest bitmap PHOINIX loads into memory (covers 2^31 clusters).
pub const MAX_BITMAP_BYTES: u64 = 256 * 1024 * 1024;

/// Allocation state of one cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClusterState {
    /// Not allocated to any file.
    Free,
    /// Allocated to some file.
    Allocated,
    /// Outside the bitmap or the bitmap is unavailable.
    Unknown,
}

/// Per-range allocation summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RangeAllocation {
    /// Clusters currently free.
    pub free: u64,
    /// Clusters currently allocated.
    pub allocated: u64,
    /// Clusters whose state could not be determined.
    pub unknown: u64,
}

impl RangeAllocation {
    /// Total clusters summarised.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.free
            .saturating_add(self.allocated)
            .saturating_add(self.unknown)
    }

    /// Accumulates another summary.
    pub fn add(&mut self, other: RangeAllocation) {
        self.free = self.free.saturating_add(other.free);
        self.allocated = self.allocated.saturating_add(other.allocated);
        self.unknown = self.unknown.saturating_add(other.unknown);
    }
}

/// Cluster allocation lookup.
pub trait ClusterAllocationMap: Send + Sync {
    /// State of one cluster.
    fn state(&self, lcn: u64) -> ClusterState;

    /// Summarises `count` clusters from `lcn`.
    fn summarize(&self, lcn: u64, count: u64) -> RangeAllocation {
        let mut out = RangeAllocation::default();
        for i in 0..count {
            match lcn
                .checked_add(i)
                .map_or(ClusterState::Unknown, |c| self.state(c))
            {
                ClusterState::Free => out.free += 1,
                ClusterState::Allocated => out.allocated += 1,
                ClusterState::Unknown => out.unknown += 1,
            }
        }
        out
    }
}

/// The volume's `$Bitmap` loaded into memory.
#[derive(Debug, Clone)]
pub struct ClusterBitmap {
    bits: Vec<u8>,
    total_clusters: u64,
}

impl ClusterBitmap {
    /// Wraps raw bitmap bytes.
    #[must_use]
    pub fn from_bytes(bits: Vec<u8>, total_clusters: u64) -> Self {
        Self {
            bits,
            total_clusters,
        }
    }

    /// Loads `$Bitmap` (record 6) from the volume.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError`] if the record or its stream cannot be read.
    pub fn load(volume: &NtfsVolume) -> Result<Self, NtfsError> {
        let file = volume.file(BITMAP_RECORD)?;
        let stream = volume.open_stream(&file, None)?;
        let bits = stream.read_all(MAX_BITMAP_BYTES)?;
        tracing::debug!(
            bytes = bits.len(),
            clusters = volume.total_clusters(),
            "$Bitmap loaded"
        );
        Ok(Self {
            bits,
            total_clusters: volume.total_clusters(),
        })
    }

    /// Number of clusters the bitmap describes.
    #[must_use]
    pub const fn total_clusters(&self) -> u64 {
        self.total_clusters
    }

    /// Number of allocated clusters in the whole volume.
    #[must_use]
    pub fn allocated_clusters(&self) -> u64 {
        let mut n = 0u64;
        for lcn in 0..self.total_clusters {
            if self.state(lcn) == ClusterState::Allocated {
                n += 1;
            }
        }
        n
    }
}

impl ClusterAllocationMap for ClusterBitmap {
    fn state(&self, lcn: u64) -> ClusterState {
        if lcn >= self.total_clusters {
            return ClusterState::Unknown;
        }
        let byte = usize::try_from(lcn / 8)
            .ok()
            .and_then(|i| self.bits.get(i).copied());
        match byte {
            None => ClusterState::Unknown,
            Some(b) => {
                if b & (1u8 << (lcn % 8)) != 0 {
                    ClusterState::Allocated
                } else {
                    ClusterState::Free
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_lookup_and_summary() {
        // clusters 0,1,2 allocated; 3..8 free; second byte all allocated
        let bm = ClusterBitmap::from_bytes(vec![0b0000_0111, 0xFF], 12);
        assert_eq!(bm.state(0), ClusterState::Allocated);
        assert_eq!(bm.state(3), ClusterState::Free);
        assert_eq!(bm.state(8), ClusterState::Allocated);
        assert_eq!(bm.state(12), ClusterState::Unknown);
        assert_eq!(
            bm.summarize(1, 4),
            RangeAllocation {
                free: 2,
                allocated: 2,
                unknown: 0
            }
        );
        assert_eq!(
            bm.summarize(10, 4),
            RangeAllocation {
                free: 0,
                allocated: 2,
                unknown: 2
            }
        );
        assert_eq!(bm.summarize(u64::MAX - 1, 4).unknown, 4);
        assert_eq!(bm.allocated_clusters(), 7);
    }
}
