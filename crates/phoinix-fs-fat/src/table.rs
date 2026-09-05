//! The File Allocation Table.

use std::collections::HashSet;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

use crate::FatError;
use crate::boot::{FatBootSector, FatVariant};

/// Largest FAT PhoinixDR loads into memory (FAT32 with 2^28 clusters is 1 GiB;
/// 256 MiB covers volumes up to 256 GiB at 4 KiB clusters).
pub const MAX_FAT_BYTES: u64 = 256 * 1024 * 1024;
/// Longest chain followed.
pub const MAX_CHAIN: usize = 1 << 26;

/// Decoded FAT entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FatEntry {
    /// Cluster is free.
    Free,
    /// Cluster is in use; the chain continues at the given cluster.
    Next(u32),
    /// Cluster is the last of its chain.
    EndOfChain,
    /// Cluster is marked bad.
    Bad,
    /// Reserved value.
    Reserved,
}

/// One FAT loaded into memory.
#[derive(Debug, Clone)]
pub struct FatTable {
    variant: FatVariant,
    cluster_count: u32,
    bytes: Vec<u8>,
    /// Whether the second FAT (if present) matched the first.
    pub mirror_consistent: Option<bool>,
}

impl FatTable {
    /// Loads the active FAT (and compares it with the mirror when present).
    ///
    /// # Errors
    ///
    /// Returns [`FatError`] if the table cannot be read or is too large.
    pub fn load(reader: &dyn BlockReader, boot: &FatBootSector) -> Result<Self, FatError> {
        if boot.fat_bytes > MAX_FAT_BYTES {
            return Err(FatError::Unsupported(format!(
                "FAT of {} bytes exceeds the in-memory limit",
                boot.fat_bytes
            )));
        }
        let len = usize::try_from(boot.fat_bytes).map_err(|_| FatError::Overflow)?;
        let active = boot.active_fat().min(boot.fat_count.saturating_sub(1));
        let bytes = read_capped(reader, boot.fat_offset_of(active), len)?;
        let mirror_consistent = if boot.fat_count > 1 {
            let other = if active == 0 { 1 } else { 0 };
            read_capped(reader, boot.fat_offset_of(other), len)
                .ok()
                .map(|m| m == bytes)
        } else {
            None
        };
        Ok(Self {
            variant: boot.variant,
            cluster_count: boot.cluster_count,
            bytes,
            mirror_consistent,
        })
    }

    /// Wraps raw FAT bytes.
    #[must_use]
    pub fn from_bytes(variant: FatVariant, cluster_count: u32, bytes: Vec<u8>) -> Self {
        Self {
            variant,
            cluster_count,
            bytes,
            mirror_consistent: None,
        }
    }

    /// Raw value of an entry, if inside the table.
    #[must_use]
    pub fn raw(&self, cluster: u32) -> Option<u32> {
        let v = ByteView::new(&self.bytes);
        let c = usize::try_from(cluster).ok()?;
        match self.variant {
            FatVariant::Fat12 => {
                let off = c + c / 2;
                let pair = v.u16_le(off)?;
                Some(u32::from(if cluster & 1 == 1 {
                    pair >> 4
                } else {
                    pair & 0x0FFF
                }))
            }
            FatVariant::Fat16 => v.u16_le(c * 2).map(u32::from),
            FatVariant::Fat32 => v.u32_le(c * 4).map(|x| x & 0x0FFF_FFFF),
        }
    }

    /// Decodes the entry for `cluster`.
    #[must_use]
    pub fn entry(&self, cluster: u32) -> FatEntry {
        let Some(raw) = self.raw(cluster) else {
            return FatEntry::Reserved;
        };
        let (eoc_min, bad) = match self.variant {
            FatVariant::Fat12 => (0xFF8, 0xFF7),
            FatVariant::Fat16 => (0xFFF8, 0xFFF7),
            FatVariant::Fat32 => (0x0FFF_FFF8, 0x0FFF_FFF7),
        };
        match raw {
            0 => FatEntry::Free,
            1 => FatEntry::Reserved,
            r if r == bad => FatEntry::Bad,
            r if r >= eoc_min => FatEntry::EndOfChain,
            r if r >= 2 && r - 2 < self.cluster_count => FatEntry::Next(r),
            _ => FatEntry::Reserved,
        }
    }

    /// Whether `cluster` is free.
    #[must_use]
    pub fn is_free(&self, cluster: u32) -> bool {
        self.entry(cluster) == FatEntry::Free
    }

    /// Follows the chain starting at `first`, protecting against loops and
    /// runaway chains.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidChain`] for loops, bad or reserved
    /// entries, or a free cluster inside the chain.
    pub fn chain(&self, first: u32) -> Result<Vec<u32>, FatError> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = first;
        loop {
            if cur < 2 || cur - 2 >= self.cluster_count {
                return Err(FatError::InvalidChain(format!(
                    "cluster {cur} outside the volume"
                )));
            }
            if !seen.insert(cur) {
                return Err(FatError::InvalidChain(format!(
                    "chain loops at cluster {cur}"
                )));
            }
            if out.len() >= MAX_CHAIN {
                return Err(FatError::InvalidChain("chain too long".into()));
            }
            out.push(cur);
            match self.entry(cur) {
                FatEntry::Next(n) => cur = n,
                FatEntry::EndOfChain => return Ok(out),
                FatEntry::Free => {
                    return Err(FatError::InvalidChain(format!(
                        "cluster {cur} is free inside a chain"
                    )));
                }
                FatEntry::Bad => {
                    return Err(FatError::InvalidChain(format!(
                        "cluster {cur} is marked bad"
                    )));
                }
                FatEntry::Reserved => {
                    return Err(FatError::InvalidChain(format!(
                        "cluster {cur} has a reserved entry"
                    )));
                }
            }
        }
    }

    /// Number of data clusters.
    #[must_use]
    pub const fn cluster_count(&self) -> u32 {
        self.cluster_count
    }

    /// Counts free clusters.
    #[must_use]
    pub fn free_clusters(&self) -> u32 {
        u32::try_from(
            (2..self.cluster_count.saturating_add(2))
                .filter(|c| self.is_free(*c))
                .count(),
        )
        .unwrap_or(u32::MAX)
    }
}

fn read_capped(reader: &dyn BlockReader, offset: u64, len: usize) -> Result<Vec<u8>, FatError> {
    let available = reader.len().saturating_sub(offset);
    let len = usize::try_from(u64::try_from(len).unwrap_or(u64::MAX).min(available))
        .map_err(|_| FatError::Overflow)?;
    let mut out = vec![0u8; len];
    reader.read_exact_at(offset, &mut out)?;
    Ok(out)
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
    fn fat12_packing() {
        // Entries: [0]=0xFF8, [1]=0xFFF, [2]=3, [3]=0xFFF, [4]=0
        // Packed 12-bit little-endian pairs.
        let entries: [u32; 5] = [0xFF8, 0xFFF, 3, 0xFFF, 0];
        let mut bytes = vec![0u8; 9];
        for (i, e) in entries.iter().enumerate() {
            let off = i + i / 2;
            let mut pair = u16::from_le_bytes([bytes[off], bytes[off + 1]]);
            if i & 1 == 1 {
                pair = (pair & 0x000F) | ((*e as u16) << 4);
            } else {
                pair = (pair & 0xF000) | (*e as u16 & 0x0FFF);
            }
            bytes[off..off + 2].copy_from_slice(&pair.to_le_bytes());
        }
        let t = FatTable::from_bytes(FatVariant::Fat12, 10, bytes);
        assert_eq!(t.entry(2), FatEntry::Next(3));
        assert_eq!(t.entry(3), FatEntry::EndOfChain);
        assert_eq!(t.entry(4), FatEntry::Free);
        assert_eq!(t.chain(2).unwrap(), vec![2, 3]);
    }

    #[test]
    fn fat32_chain_and_errors() {
        let mut bytes = vec![0u8; 4 * 16];
        let set = |b: &mut Vec<u8>, c: usize, v: u32| {
            b[c * 4..c * 4 + 4].copy_from_slice(&v.to_le_bytes())
        };
        set(&mut bytes, 2, 5);
        set(&mut bytes, 5, 3);
        set(&mut bytes, 3, 0x0FFF_FFFF);
        set(&mut bytes, 6, 6); // self loop
        set(&mut bytes, 7, 0x0FFF_FFF7); // bad
        set(&mut bytes, 8, 9);
        let t = FatTable::from_bytes(FatVariant::Fat32, 14, bytes);
        assert_eq!(t.chain(2).unwrap(), vec![2, 5, 3]);
        assert!(t.chain(6).is_err());
        assert_eq!(t.entry(7), FatEntry::Bad);
        assert!(t.chain(8).is_err(), "chain into a free cluster");
        assert!(t.chain(100).is_err());
        assert!(t.is_free(4));
        assert_eq!(t.free_clusters(), 14 - 6);
    }
}
