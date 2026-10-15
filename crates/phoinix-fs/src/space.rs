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

    /// The slack of the allocated files: for every active file, the bytes
    /// between the end of its content and the end of the last cluster
    /// allocated to it. Deleted files, resident data, compressed, sparse
    /// and encrypted streams and files whose layout could not be read in
    /// full are left out, so that an overwrite of these ranges never
    /// touches live data. `None` when the engine cannot enumerate the
    /// allocated files.
    ///
    /// # Errors
    ///
    /// Returns [`FsError`] if the directory tree cannot be read.
    fn slack_ranges(&self) -> Result<Option<Vec<ByteRange>>, FsError> {
        Ok(None)
    }
}

/// A run of a file's content: `length` bytes at logical offset `logical`
/// stored at volume offset `physical`. Input of [`tail_of_runs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentRun {
    /// Logical byte offset inside the file.
    pub logical: u64,
    /// Volume byte offset of the stored bytes.
    pub physical: u64,
    /// Length in bytes.
    pub length: u64,
}

/// The slack of one file: the stored bytes at logical offsets of
/// `logical_size` and beyond, in the runs' order, adjacent pieces merged.
/// Helper for engines implementing [`AllocationView::slack_ranges`].
#[must_use]
pub fn tail_of_runs(runs: &[ContentRun], logical_size: u64) -> Vec<ByteRange> {
    let mut out: Vec<ByteRange> = Vec::new();
    for r in runs {
        let end = r.logical.saturating_add(r.length);
        if end <= logical_size || r.length == 0 {
            continue;
        }
        let skip = logical_size.saturating_sub(r.logical);
        let range = ByteRange {
            offset: r.physical.saturating_add(skip),
            length: r.length - skip,
        };
        match out.last_mut() {
            Some(last) if last.end() == range.offset => last.length += range.length,
            _ => out.push(range),
        }
    }
    out
}

/// Runs for clusters stored contiguously in logical order (FAT chains):
/// adjacent clusters become one run.
#[must_use]
pub fn runs_of_clusters(cluster_offsets: &[u64], cluster_size: u64) -> Vec<ContentRun> {
    let cluster_size = cluster_size.max(1);
    let mut out: Vec<ContentRun> = Vec::new();
    let mut logical = 0u64;
    for off in cluster_offsets {
        match out.last_mut() {
            Some(last) if last.physical.saturating_add(last.length) == *off => {
                last.length = last.length.saturating_add(cluster_size);
            }
            _ => out.push(ContentRun {
                logical,
                physical: *off,
                length: cluster_size,
            }),
        }
        logical = logical.saturating_add(cluster_size);
    }
    out
}

/// Sorts and merges byte ranges (adjacent and overlapping ones become one).
#[must_use]
pub fn merge_ranges(mut ranges: Vec<ByteRange>) -> Vec<ByteRange> {
    ranges.retain(|r| r.length > 0);
    ranges.sort_by_key(|r| r.offset);
    let mut out: Vec<ByteRange> = Vec::with_capacity(ranges.len());
    for r in ranges {
        match out.last_mut() {
            Some(last) if r.offset <= last.end() => {
                let end = last.end().max(r.end());
                last.length = end - last.offset;
            }
            _ => out.push(r),
        }
    }
    out
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
    fn tail_of_runs_covers_only_the_bytes_past_the_content() {
        // Clusters of 512 bytes at 4096, 8192, 2048; content 1100 bytes:
        // cluster 2 holds bytes 1024..1536, of which 1100..1536 is slack.
        let runs = runs_of_clusters(&[4096, 8192, 2048], 512);
        assert_eq!(runs.len(), 3);
        assert_eq!(
            tail_of_runs(&runs, 1100),
            vec![ByteRange {
                offset: 2048 + 76,
                length: 436
            }]
        );
        // Contiguous clusters merge into one run; a size on a cluster
        // boundary leaves no slack.
        let runs = runs_of_clusters(&[4096, 4608, 5120], 512);
        assert_eq!(
            runs,
            vec![ContentRun {
                logical: 0,
                physical: 4096,
                length: 1536
            }]
        );
        assert!(tail_of_runs(&runs, 1536).is_empty());
        // Whole preallocated clusters past the content are slack too.
        assert_eq!(
            tail_of_runs(&runs, 600),
            vec![ByteRange {
                offset: 4096 + 600,
                length: 936
            }]
        );
        // A hole (no run) past the content contributes nothing; a run
        // entirely past the content is slack in full.
        let runs = [
            ContentRun {
                logical: 0,
                physical: 10_000,
                length: 1024,
            },
            ContentRun {
                logical: 4096,
                physical: 20_000,
                length: 1024,
            },
        ];
        assert_eq!(
            tail_of_runs(&runs, 1000),
            vec![
                ByteRange {
                    offset: 11_000,
                    length: 24
                },
                ByteRange {
                    offset: 20_000,
                    length: 1024
                }
            ]
        );
        assert!(tail_of_runs(&[], 0).is_empty());
    }

    #[test]
    fn merge_ranges_sorts_and_joins() {
        let m = merge_ranges(vec![
            ByteRange {
                offset: 100,
                length: 10,
            },
            ByteRange {
                offset: 0,
                length: 50,
            },
            ByteRange {
                offset: 40,
                length: 20,
            },
            ByteRange {
                offset: 60,
                length: 0,
            },
        ]);
        assert_eq!(
            m,
            vec![
                ByteRange {
                    offset: 0,
                    length: 60
                },
                ByteRange {
                    offset: 100,
                    length: 10
                }
            ]
        );
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
