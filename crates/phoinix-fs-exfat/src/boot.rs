//! exFAT boot sector and boot-region checksum.

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::arith;
use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

use crate::ExfatError;

/// Parsed and validated exFAT boot sector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExfatBootSector {
    /// Sector offset of the partition on the disk (informational).
    pub partition_offset: u64,
    /// Volume length in sectors.
    pub volume_length: u64,
    /// FAT offset in sectors.
    pub fat_offset: u32,
    /// FAT length in sectors.
    pub fat_length: u32,
    /// Cluster heap offset in sectors.
    pub cluster_heap_offset: u32,
    /// Number of clusters in the heap.
    pub cluster_count: u32,
    /// First cluster of the root directory.
    pub root_cluster: u32,
    /// Volume serial number.
    pub volume_serial: u32,
    /// Filesystem revision (major, minor).
    pub revision: (u8, u8),
    /// Volume flags (bit 0 active FAT, bit 1 dirty, bit 2 media failure).
    pub volume_flags: u16,
    /// Bytes per sector.
    pub bytes_per_sector: u32,
    /// Sectors per cluster.
    pub sectors_per_cluster: u32,
    /// Number of FATs.
    pub fat_count: u8,
    /// Percent of clusters in use as recorded.
    pub percent_in_use: u8,
    /// Cluster size in bytes.
    pub cluster_size: u32,
    /// Byte offset of cluster 2.
    pub heap_offset: u64,
}

impl ExfatBootSector {
    /// Parses the first 512 bytes of a volume.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError::InvalidBootSector`] describing the first failed
    /// check.
    pub fn parse(sector: &[u8]) -> Result<Self, ExfatError> {
        let v = ByteView::new(sector);
        let invalid = |what: &str| ExfatError::InvalidBootSector(what.to_owned());
        if v.slice(3, 8) != Some(b"EXFAT   ") {
            return Err(invalid("OEM name is not EXFAT"));
        }
        if !v.slice(11, 53).is_some_and(|z| z.iter().all(|b| *b == 0)) {
            return Err(invalid("must-be-zero region is not zero"));
        }
        let partition_offset = v.u64_le(64).ok_or_else(|| invalid("truncated"))?;
        let volume_length = v.u64_le(72).ok_or_else(|| invalid("truncated"))?;
        let fat_offset = v.u32_le(80).ok_or_else(|| invalid("truncated"))?;
        let fat_length = v.u32_le(84).ok_or_else(|| invalid("truncated"))?;
        let cluster_heap_offset = v.u32_le(88).ok_or_else(|| invalid("truncated"))?;
        let cluster_count = v.u32_le(92).ok_or_else(|| invalid("truncated"))?;
        let root_cluster = v.u32_le(96).ok_or_else(|| invalid("truncated"))?;
        let volume_serial = v.u32_le(100).ok_or_else(|| invalid("truncated"))?;
        let revision = (v.u8(105).unwrap_or(0), v.u8(104).unwrap_or(0));
        let volume_flags = v.u16_le(106).ok_or_else(|| invalid("truncated"))?;
        let bps_shift = v.u8(108).ok_or_else(|| invalid("truncated"))?;
        let spc_shift = v.u8(109).ok_or_else(|| invalid("truncated"))?;
        let fat_count = v.u8(110).ok_or_else(|| invalid("truncated"))?;
        let percent_in_use = v.u8(112).ok_or_else(|| invalid("truncated"))?;
        if !(9..=12).contains(&bps_shift) {
            return Err(ExfatError::InvalidBootSector(format!(
                "bytes-per-sector shift {bps_shift} invalid"
            )));
        }
        if spc_shift > 25 - bps_shift {
            return Err(ExfatError::InvalidBootSector(format!(
                "sectors-per-cluster shift {spc_shift} invalid"
            )));
        }
        if !(1..=2).contains(&fat_count) {
            return Err(ExfatError::InvalidBootSector(format!(
                "FAT count {fat_count} invalid"
            )));
        }
        if v.u16_le(510) != Some(0xAA55) {
            return Err(invalid("missing 55 AA signature"));
        }
        if fat_offset < 24 || fat_length == 0 || cluster_count == 0 || cluster_heap_offset == 0 {
            return Err(invalid("region offsets are invalid"));
        }
        if root_cluster < 2 || root_cluster - 2 >= cluster_count {
            return Err(ExfatError::InvalidBootSector(format!(
                "root cluster {root_cluster} outside the heap"
            )));
        }
        let bytes_per_sector = 1u32 << bps_shift;
        let sectors_per_cluster = 1u32 << spc_shift;
        let cluster_size = bytes_per_sector << spc_shift;
        let heap_offset = arith::mul(u64::from(cluster_heap_offset), u64::from(bytes_per_sector))?;
        if u64::from(cluster_heap_offset) >= volume_length {
            return Err(invalid("cluster heap starts beyond the volume"));
        }
        Ok(Self {
            partition_offset,
            volume_length,
            fat_offset,
            fat_length,
            cluster_heap_offset,
            cluster_count,
            root_cluster,
            volume_serial,
            revision,
            volume_flags,
            bytes_per_sector,
            sectors_per_cluster,
            fat_count,
            percent_in_use,
            cluster_size,
            heap_offset,
        })
    }

    /// Whether `cluster` is a valid heap cluster.
    #[must_use]
    pub const fn is_valid_cluster(&self, cluster: u32) -> bool {
        cluster >= 2 && cluster - 2 < self.cluster_count
    }

    /// Volume byte offset of `cluster`.
    ///
    /// # Errors
    ///
    /// Returns [`ExfatError::Malformed`] for clusters outside the heap.
    pub fn cluster_offset(&self, cluster: u32) -> Result<u64, ExfatError> {
        if !self.is_valid_cluster(cluster) {
            return Err(ExfatError::Malformed(format!(
                "cluster {cluster} is outside the heap"
            )));
        }
        Ok(arith::add(
            self.heap_offset,
            arith::mul(u64::from(cluster - 2), u64::from(self.cluster_size))?,
        )?)
    }

    /// Byte offset of the active FAT.
    #[must_use]
    pub fn fat_byte_offset(&self) -> u64 {
        let active = if self.fat_count == 2 && self.volume_flags & 1 != 0 {
            1u64
        } else {
            0
        };
        (u64::from(self.fat_offset) + active * u64::from(self.fat_length))
            * u64::from(self.bytes_per_sector)
    }

    /// Byte length of one FAT.
    #[must_use]
    pub fn fat_bytes(&self) -> u64 {
        u64::from(self.fat_length) * u64::from(self.bytes_per_sector)
    }

    /// Volume size in bytes as declared.
    #[must_use]
    pub fn volume_bytes(&self) -> u64 {
        self.volume_length
            .saturating_mul(u64::from(self.bytes_per_sector))
    }

    /// Verifies the boot-region checksum stored in sector 11 over sectors
    /// 0–10, returning whether it matches (or `None` if it cannot be read).
    #[must_use]
    pub fn verify_region_checksum(&self, reader: &dyn BlockReader) -> Option<bool> {
        let bps = usize::try_from(self.bytes_per_sector).ok()?;
        let region = reader.read_vec(0, bps * 12).ok()?;
        let mut sum: u32 = 0;
        for (i, b) in region.iter().take(bps * 11).enumerate() {
            if i == 106 || i == 107 || i == 112 {
                continue;
            }
            sum = sum.rotate_right(1).wrapping_add(u32::from(*b));
        }
        let stored = ByteView::new(&region).u32_le(bps * 11)?;
        Some(stored == sum)
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Synthetic exFAT boot sector.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    pub fn boot(volume_sectors: u64, cluster_count: u32, spc_shift: u8) -> Vec<u8> {
        let mut s = vec![0u8; 512];
        s[0] = 0xEB;
        s[1] = 0x76;
        s[2] = 0x90;
        s[3..11].copy_from_slice(b"EXFAT   ");
        s[72..80].copy_from_slice(&volume_sectors.to_le_bytes());
        s[80..84].copy_from_slice(&2048u32.to_le_bytes());
        s[84..88].copy_from_slice(&128u32.to_le_bytes());
        s[88..92].copy_from_slice(&4096u32.to_le_bytes());
        s[92..96].copy_from_slice(&cluster_count.to_le_bytes());
        s[96..100].copy_from_slice(&5u32.to_le_bytes());
        s[100..104].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        s[104] = 0;
        s[105] = 1;
        s[108] = 9;
        s[109] = spc_shift;
        s[110] = 1;
        s[510] = 0x55;
        s[511] = 0xAA;
        s
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

    use super::testutil::boot;
    use super::*;

    #[test]
    fn parses_geometry() {
        let b = ExfatBootSector::parse(&boot(131_072, 15_872, 3)).unwrap();
        assert_eq!(b.bytes_per_sector, 512);
        assert_eq!(b.cluster_size, 4096);
        assert_eq!(b.heap_offset, 4096 * 512);
        assert_eq!(b.cluster_offset(2).unwrap(), b.heap_offset);
        assert_eq!(b.cluster_offset(5).unwrap(), b.heap_offset + 3 * 4096);
        assert!(b.cluster_offset(1).is_err());
        assert_eq!(b.fat_byte_offset(), 2048 * 512);
        assert_eq!(b.revision, (1, 0));
    }

    #[test]
    fn rejects_bad_sectors() {
        let mut s = boot(131_072, 15_872, 3);
        s[108] = 13;
        assert!(ExfatBootSector::parse(&s).is_err());
        let mut s = boot(131_072, 15_872, 3);
        s[20] = 1;
        assert!(ExfatBootSector::parse(&s).is_err());
        let mut s = boot(131_072, 15_872, 3);
        s[96..100].copy_from_slice(&1u32.to_le_bytes());
        assert!(ExfatBootSector::parse(&s).is_err());
        assert!(ExfatBootSector::parse(&vec![0u8; 512]).is_err());
    }
}
