//! The exFAT File Allocation Table (only used by files without the
//! `NoFatChain` flag).

use std::collections::HashSet;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::bytes::ByteView;

use crate::ExfatError;
use crate::boot::ExfatBootSector;

/// Largest FAT loaded into memory.
pub const MAX_FAT_BYTES: u64 = 256 * 1024 * 1024;
/// Longest chain followed.
pub const MAX_CHAIN: usize = 1 << 26;
/// End-of-chain marker.
pub const END_OF_CHAIN: u32 = 0xFFFF_FFFF;
/// Bad-cluster marker.
pub const BAD_CLUSTER: u32 = 0xFFFF_FFF7;

/// The FAT in memory.
#[derive(Debug, Clone)]
pub struct ExfatTable {
    bytes: Vec<u8>,
    cluster_count: u32,
}

impl ExfatTable {
    /// Loads the active FAT.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError`] if it cannot be read or is too large.
    pub fn load(reader: &dyn BlockReader, boot: &ExfatBootSector) -> Result<Self, ExfatError> {
        let len = boot.fat_bytes();
        if len > MAX_FAT_BYTES {
            return Err(ExfatError::Unsupported(format!(
                "FAT of {len} bytes exceeds the in-memory limit"
            )));
        }
        let available = reader.len().saturating_sub(boot.fat_byte_offset()).min(len);
        let mut bytes = vec![0u8; usize::try_from(available).map_err(|_| ExfatError::Overflow)?];
        reader.read_exact_at(boot.fat_byte_offset(), &mut bytes)?;
        Ok(Self {
            bytes,
            cluster_count: boot.cluster_count,
        })
    }

    /// Wraps raw bytes.
    #[must_use]
    pub fn from_bytes(bytes: Vec<u8>, cluster_count: u32) -> Self {
        Self {
            bytes,
            cluster_count,
        }
    }

    /// Raw entry for `cluster`.
    #[must_use]
    pub fn raw(&self, cluster: u32) -> Option<u32> {
        ByteView::new(&self.bytes).u32_le(usize::try_from(cluster).ok()?.checked_mul(4)?)
    }

    /// Follows the chain from `first` for at most `max_clusters` clusters.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError::Malformed`] for loops, bad clusters, free
    /// entries inside the chain or clusters outside the heap.
    pub fn chain(&self, first: u32, max_clusters: u64) -> Result<Vec<u32>, ExfatError> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = first;
        loop {
            if cur < 2 || cur - 2 >= self.cluster_count {
                return Err(ExfatError::Malformed(format!(
                    "cluster {cur} outside the heap"
                )));
            }
            if !seen.insert(cur) {
                return Err(ExfatError::Malformed(format!(
                    "chain loops at cluster {cur}"
                )));
            }
            out.push(cur);
            if out.len() as u64 >= max_clusters || out.len() >= MAX_CHAIN {
                return Ok(out);
            }
            match self.raw(cur) {
                Some(END_OF_CHAIN) => return Ok(out),
                Some(0) => {
                    return Err(ExfatError::Malformed(format!(
                        "cluster {cur} has a free FAT entry inside a chain"
                    )));
                }
                Some(BAD_CLUSTER) => {
                    return Err(ExfatError::Malformed(format!(
                        "cluster {cur} is marked bad"
                    )));
                }
                Some(next) => cur = next,
                None => return Err(ExfatError::Malformed("FAT truncated".into())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn chains() {
        let mut bytes = vec![0u8; 4 * 16];
        let set = |b: &mut Vec<u8>, c: usize, v: u32| {
            b[c * 4..c * 4 + 4].copy_from_slice(&v.to_le_bytes())
        };
        set(&mut bytes, 2, 7);
        set(&mut bytes, 7, 3);
        set(&mut bytes, 3, END_OF_CHAIN);
        set(&mut bytes, 4, 4);
        let t = ExfatTable::from_bytes(bytes, 14);
        assert_eq!(t.chain(2, 100).unwrap(), vec![2, 7, 3]);
        assert_eq!(t.chain(2, 2).unwrap(), vec![2, 7]);
        assert!(t.chain(4, 100).is_err());
        assert!(t.chain(5, 100).is_err(), "free entry");
    }
}
