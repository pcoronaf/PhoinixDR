//! Mapping-pairs (runlist) decoding.
//!
//! LCN deltas are signed and relative to the previous run; a decoder using
//! unsigned arithmetic silently corrupts fragmented files.

use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

use crate::NtfsError;

/// One run of a non-resident attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NtfsRun {
    /// Clusters stored on disk.
    Data {
        /// First VCN of the run.
        vcn: u64,
        /// First LCN of the run.
        lcn: u64,
        /// Number of clusters.
        clusters: u64,
    },
    /// Clusters that read as zero and occupy no disk space.
    Sparse {
        /// First VCN of the run.
        vcn: u64,
        /// Number of clusters.
        clusters: u64,
    },
}

impl NtfsRun {
    /// First VCN.
    #[must_use]
    pub const fn vcn(&self) -> u64 {
        match self {
            NtfsRun::Data { vcn, .. } | NtfsRun::Sparse { vcn, .. } => *vcn,
        }
    }

    /// Cluster count.
    #[must_use]
    pub const fn clusters(&self) -> u64 {
        match self {
            NtfsRun::Data { clusters, .. } | NtfsRun::Sparse { clusters, .. } => *clusters,
        }
    }

    /// Whether the run is sparse.
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        matches!(self, NtfsRun::Sparse { .. })
    }

    /// One-past-the-last VCN.
    #[must_use]
    pub const fn end_vcn(&self) -> u64 {
        self.vcn().saturating_add(self.clusters())
    }
}

/// Decodes mapping pairs into runs.
///
/// `starting_vcn` is the attribute's starting VCN; `total_clusters` bounds
/// every LCN.
///
/// # Errors
///
/// Returns [`NtfsError::InvalidRunlist`] for zero-length runs, invalid field
/// widths, truncated pairs, VCN overflow, LCN underflow, or LCNs outside the
/// volume.
pub fn decode_runlist(
    mapping_pairs: &[u8],
    starting_vcn: u64,
    total_clusters: u64,
) -> Result<Vec<NtfsRun>, NtfsError> {
    let view = ByteView::new(mapping_pairs);
    let mut runs = Vec::new();
    let mut pos = 0usize;
    let mut vcn = starting_vcn;
    let mut lcn: i128 = 0;
    loop {
        let header = view
            .u8(pos)
            .ok_or_else(|| NtfsError::InvalidRunlist("truncated: missing terminator".into()))?;
        if header == 0 {
            break;
        }
        let length_size = usize::from(header & 0x0F);
        let offset_size = usize::from(header >> 4);
        if length_size == 0 || length_size > 8 || offset_size > 8 {
            return Err(NtfsError::InvalidRunlist(format!(
                "invalid field widths in header byte {header:#04x} at {pos}"
            )));
        }
        let clusters = view
            .uint_le(pos + 1, length_size)
            .ok_or_else(|| NtfsError::InvalidRunlist("truncated run length".into()))?;
        if clusters == 0 {
            return Err(NtfsError::InvalidRunlist(format!(
                "zero-length run at {pos}"
            )));
        }
        let end_vcn = vcn
            .checked_add(clusters)
            .ok_or_else(|| NtfsError::InvalidRunlist("VCN overflow".into()))?;
        if offset_size == 0 {
            runs.push(NtfsRun::Sparse { vcn, clusters });
        } else {
            let delta = view
                .int_le(pos + 1 + length_size, offset_size)
                .ok_or_else(|| NtfsError::InvalidRunlist("truncated run offset".into()))?;
            lcn += i128::from(delta);
            if lcn < 0 {
                return Err(NtfsError::InvalidRunlist(format!("LCN underflow at {pos}")));
            }
            let lcn_u =
                u64::try_from(lcn).map_err(|_| NtfsError::InvalidRunlist("LCN overflow".into()))?;
            let run_end = lcn_u
                .checked_add(clusters)
                .ok_or_else(|| NtfsError::InvalidRunlist("LCN overflow".into()))?;
            if run_end > total_clusters {
                return Err(NtfsError::InvalidRunlist(format!(
                    "run at LCN {lcn_u} with {clusters} clusters extends beyond the volume ({total_clusters} clusters)"
                )));
            }
            runs.push(NtfsRun::Data {
                vcn,
                lcn: lcn_u,
                clusters,
            });
        }
        vcn = end_vcn;
        pos = pos
            .checked_add(1 + length_size + offset_size)
            .ok_or_else(|| NtfsError::InvalidRunlist("position overflow".into()))?;
    }
    Ok(runs)
}

/// Total clusters covered by a runlist.
#[must_use]
pub fn total_clusters(runs: &[NtfsRun]) -> u64 {
    runs.iter()
        .fold(0u64, |acc, r| acc.saturating_add(r.clusters()))
}

/// Number of physical extents (data runs) in a runlist.
#[must_use]
pub fn extent_count(runs: &[NtfsRun]) -> u32 {
    u32::try_from(runs.iter().filter(|r| !r.is_sparse()).count()).unwrap_or(u32::MAX)
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
    fn single_extent() {
        // length 0x08, LCN +0x20
        let runs = decode_runlist(&[0x11, 0x08, 0x20, 0x00], 0, 1000).unwrap();
        assert_eq!(
            runs,
            vec![NtfsRun::Data {
                vcn: 0,
                lcn: 0x20,
                clusters: 8
            }]
        );
    }

    #[test]
    fn multiple_extents_with_negative_delta() {
        // VCN 0 -> LCN 1000 len 20; VCN 20 -> LCN 800 len 10 (delta -200)
        let mut pairs = vec![0x21, 20, 0xE8, 0x03]; // len 1 byte = 20, off 2 bytes = 1000
        pairs.extend_from_slice(&[0x21, 10, 0x38, 0xFF]); // len 10, off -200 (0xFF38)
        pairs.push(0);
        let runs = decode_runlist(&pairs, 0, 5000).unwrap();
        assert_eq!(
            runs,
            vec![
                NtfsRun::Data {
                    vcn: 0,
                    lcn: 1000,
                    clusters: 20
                },
                NtfsRun::Data {
                    vcn: 20,
                    lcn: 800,
                    clusters: 10
                }
            ]
        );
        assert_eq!(total_clusters(&runs), 30);
        assert_eq!(extent_count(&runs), 2);
    }

    #[test]
    fn sparse_run_and_large_extent() {
        let mut pairs = vec![0x01, 5]; // sparse 5 clusters
        pairs.extend_from_slice(&[0x33, 0x00, 0x00, 0x10, 0x00, 0x00, 0x01]); // len 0x100000, off 0x010000
        pairs.push(0);
        let runs = decode_runlist(&pairs, 0, 1 << 40).unwrap();
        assert_eq!(
            runs[0],
            NtfsRun::Sparse {
                vcn: 0,
                clusters: 5
            }
        );
        assert_eq!(
            runs[1],
            NtfsRun::Data {
                vcn: 5,
                lcn: 0x1_0000,
                clusters: 0x10_0000
            }
        );
        assert_eq!(extent_count(&runs), 1);
    }

    #[test]
    fn rejects_malformed() {
        assert!(decode_runlist(&[0x11, 0x08], 0, 100).is_err(), "truncated");
        assert!(
            decode_runlist(&[0x11, 0x08, 0x20], 0, 100).is_err(),
            "missing terminator"
        );
        assert!(
            decode_runlist(&[0x11, 0x00, 0x20, 0x00], 0, 100).is_err(),
            "zero length"
        );
        assert!(
            decode_runlist(&[0x19, 0x08, 0x20, 0x00], 0, 100).is_err(),
            "bad width"
        );
        assert!(
            decode_runlist(&[0x10, 0x20, 0x00], 0, 100).is_err(),
            "zero length width"
        );
        assert!(
            decode_runlist(&[0x11, 0x08, 0xF0, 0x00], 0, 100).is_err(),
            "LCN underflow"
        );
        assert!(
            decode_runlist(&[0x11, 0x08, 0x60, 0x00], 0, 100).is_err(),
            "LCN outside volume"
        );
        assert!(
            decode_runlist(
                &[
                    0x18, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x00
                ],
                5,
                u64::MAX
            )
            .is_err(),
            "VCN overflow"
        );
    }
}
