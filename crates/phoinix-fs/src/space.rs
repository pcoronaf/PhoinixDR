//! Allocation views: what a filesystem engine knows about which bytes of
//! its volume are free. Deep scan (carving) iterates the free space and
//! scores carved files against the same allocation evidence the metadata
//! engines use.

use serde::{Deserialize, Serialize};

use crate::FsError;

/// A contiguous byte range of a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    /// Volume byte offset.
    pub offset: u64,
    /// Length in bytes.
    pub length: u64,
}

impl ByteRange {
    /// Exclusive end offset (saturating).
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.offset.saturating_add(self.length)
    }
}

/// Allocation state of a byte range, in clusters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AllocationSummary {
    /// Clusters currently free.
    pub free: u64,
    /// Clusters currently allocated to active data.
    pub allocated: u64,
    /// Clusters whose state is unknown (no map, or outside it).
    pub unknown: u64,
}

impl AllocationSummary {
    /// Total clusters summarised.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.free
            .saturating_add(self.allocated)
            .saturating_add(self.unknown)
    }
}

/// What an engine knows about the allocation of its volume.
///
/// Byte ranges and cluster counts are expressed for the volume the engine
/// was opened on (offset 0 = start of the volume).
pub trait AllocationView: Send + Sync {
    /// Allocation unit in bytes (cluster size), at least 1.
    fn cluster_size(&self) -> u64;

    /// Length of the volume in bytes.
    fn volume_len(&self) -> u64;

    /// Whether an allocation map is available at all. Without one every
    /// range is `unknown` and [`free_ranges`](Self::free_ranges) returns the
    /// whole volume.
    fn map_available(&self) -> bool;

    /// Free byte ranges of the volume, sorted and merged.
    ///
    /// # Errors
    ///
    /// Returns [`FsError`] if the allocation structures cannot be read.
    fn free_ranges(&self) -> Result<Vec<ByteRange>, FsError>;

    /// Summarises the clusters overlapping `range`.
    fn summarize(&self, range: ByteRange) -> AllocationSummary;
}

/// Merges sorted cluster states into byte ranges: helper for engines that
/// expose a per-cluster query.
///
/// `state(cluster)` returns `Some(true)` for a free cluster, `Some(false)`
/// for an allocated one and `None` for unknown; unknown clusters are
/// treated as free so that carving still covers them.
#[must_use]
pub fn free_ranges_from<F>(
    cluster_count: u64,
    cluster_size: u64,
    data_offset: u64,
    state: F,
) -> Vec<ByteRange>
where
    F: Fn(u64) -> Option<bool>,
{
    let cluster_size = cluster_size.max(1);
    let mut out: Vec<ByteRange> = Vec::new();
    let mut run_start: Option<u64> = None;
    let flush = |out: &mut Vec<ByteRange>, start: u64, end: u64| {
        let offset = data_offset.saturating_add(start.saturating_mul(cluster_size));
        let length = end.saturating_sub(start).saturating_mul(cluster_size);
        if length > 0 {
            out.push(ByteRange { offset, length });
        }
    };
    for c in 0..cluster_count {
        let free = state(c).unwrap_or(true);
        match (free, run_start) {
            (true, None) => run_start = Some(c),
            (false, Some(start)) => {
                flush(&mut out, start, c);
                run_start = None;
            }
            _ => {}
        }
    }
    if let Some(start) = run_start {
        flush(&mut out, start, cluster_count);
    }
    out
}

/// Summarises the clusters of `range` with a per-cluster query.
#[must_use]
pub fn summarize_with<F>(
    range: ByteRange,
    cluster_size: u64,
    data_offset: u64,
    cluster_count: u64,
    state: F,
) -> AllocationSummary
where
    F: Fn(u64) -> Option<bool>,
{
    let cluster_size = cluster_size.max(1);
    let mut out = AllocationSummary::default();
    if range.length == 0 {
        return out;
    }
    let first = range.offset.saturating_sub(data_offset) / cluster_size;
    let last = range.end().saturating_sub(1).saturating_sub(data_offset) / cluster_size;
    for c in first..=last {
        if range.offset < data_offset || c >= cluster_count {
            out.unknown += 1;
            continue;
        }
        match state(c) {
            Some(true) => out.free += 1,
            Some(false) => out.allocated += 1,
            None => out.unknown += 1,
        }
    }
    out
}

/// An allocation view for sources without a usable filesystem: everything
/// is unknown and the whole source is scanned.
#[derive(Debug, Clone)]
pub struct WholeSource {
    len: u64,
    cluster_size: u64,
}

impl WholeSource {
    /// A view over `len` bytes with `cluster_size` as the nominal unit.
    #[must_use]
    pub const fn new(len: u64, cluster_size: u64) -> Self {
        Self { len, cluster_size }
    }
}

impl AllocationView for WholeSource {
    fn cluster_size(&self) -> u64 {
        self.cluster_size.max(1)
    }

    fn volume_len(&self) -> u64 {
        self.len
    }

    fn map_available(&self) -> bool {
        false
    }

    fn free_ranges(&self) -> Result<Vec<ByteRange>, FsError> {
        Ok(vec![ByteRange {
            offset: 0,
            length: self.len,
        }])
    }

    fn summarize(&self, range: ByteRange) -> AllocationSummary {
        summarize_with(range, self.cluster_size(), 0, u64::MAX, |_| None)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]
    use super::*;

    #[test]
    fn merges_free_clusters_into_ranges() {
        // clusters: F F A F A A F F (8 clusters of 512 bytes from offset 4096)
        let states = [true, true, false, true, false, false, true, true];
        let ranges = free_ranges_from(8, 512, 4096, |c| states.get(c as usize).copied());
        assert_eq!(
            ranges,
            vec![
                ByteRange {
                    offset: 4096,
                    length: 1024
                },
                ByteRange {
                    offset: 4096 + 1536,
                    length: 512
                },
                ByteRange {
                    offset: 4096 + 3072,
                    length: 1024
                },
            ]
        );
        let s = summarize_with(
            ByteRange {
                offset: 4096 + 600,
                length: 2000,
            },
            512,
            4096,
            8,
            |c| states.get(c as usize).copied(),
        );
        // bytes 600..2600 → clusters 1..=5: F A F A A
        assert_eq!(
            s,
            AllocationSummary {
                free: 2,
                allocated: 3,
                unknown: 0
            }
        );
        assert_eq!(s.total(), 5);
    }

    #[test]
    fn whole_source_is_unknown_everywhere() {
        let w = WholeSource::new(10_000, 512);
        assert!(!w.map_available());
        assert_eq!(w.free_ranges().unwrap().len(), 1);
        let s = w.summarize(ByteRange {
            offset: 0,
            length: 1024,
        });
        assert_eq!(s.unknown, 2);
    }
}
