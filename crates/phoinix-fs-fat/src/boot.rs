//! FAT boot sector / BIOS Parameter Block.

use phoinix_core::arith;
use phoinix_core::bytes::{ByteView, ascii_field};
use serde::{Deserialize, Serialize};

use crate::FatError;

/// FAT variant, decided by the number of data clusters as the
/// specification requires (never by the type label).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FatVariant {
    /// FAT12.
    Fat12,
    /// FAT16.
    Fat16,
    /// FAT32.
    Fat32,
}

impl FatVariant {
    /// Label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            FatVariant::Fat12 => "FAT12",
            FatVariant::Fat16 => "FAT16",
            FatVariant::Fat32 => "FAT32",
        }
    }

    /// The corresponding [`phoinix_core::FileSystemType`].
    #[must_use]
    pub const fn filesystem_type(&self) -> phoinix_core::FileSystemType {
        match self {
            FatVariant::Fat12 => phoinix_core::FileSystemType::Fat12,
            FatVariant::Fat16 => phoinix_core::FileSystemType::Fat16,
            FatVariant::Fat32 => phoinix_core::FileSystemType::Fat32,
        }
    }
}

impl std::fmt::Display for FatVariant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Parsed and validated boot sector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FatBootSector {
    /// OEM name.
    pub oem_name: String,
    /// Bytes per sector.
    pub bytes_per_sector: u16,
    /// Sectors per cluster.
    pub sectors_per_cluster: u8,
    /// Reserved sectors before the first FAT.
    pub reserved_sectors: u16,
    /// Number of FATs.
    pub fat_count: u8,
    /// Root directory entries (FAT12/16 only; zero on FAT32).
    pub root_entries: u16,
    /// Total sectors.
    pub total_sectors: u64,
    /// Media descriptor.
    pub media: u8,
    /// Sectors per FAT.
    pub sectors_per_fat: u32,
    /// Hidden sectors before the volume.
    pub hidden_sectors: u32,
    /// FAT32 root directory cluster (2 for FAT12/16 pseudo).
    pub root_cluster: u32,
    /// FAT32 FSInfo sector, if any.
    pub fs_info_sector: Option<u16>,
    /// FAT32 extended flags (active FAT, mirroring).
    pub ext_flags: u16,
    /// Volume serial number.
    pub volume_serial: u32,
    /// Volume label from the BPB.
    pub volume_label: String,
    /// Filesystem type label from the BPB (informational only).
    pub type_label: String,
    /// Variant decided from the cluster count.
    pub variant: FatVariant,
    /// Number of data clusters (clusters 2..2+count).
    pub cluster_count: u32,
    /// Cluster size in bytes.
    pub cluster_size: u32,
    /// Byte offset of the first FAT.
    pub fat_offset: u64,
    /// Byte length of one FAT.
    pub fat_bytes: u64,
    /// Byte offset of the fixed root directory (FAT12/16).
    pub root_dir_offset: u64,
    /// Byte length of the fixed root directory (FAT12/16).
    pub root_dir_bytes: u64,
    /// Byte offset of cluster 2.
    pub data_offset: u64,
}

impl FatBootSector {
    /// Parses the first 512 bytes of a volume.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidBootSector`] describing the first failed
    /// check.
    pub fn parse(sector: &[u8]) -> Result<Self, FatError> {
        let v = ByteView::new(sector);
        let invalid = |what: &str| FatError::InvalidBootSector(what.to_owned());
        let jump = v.array::<3>(0).ok_or_else(|| invalid("truncated"))?;
        if !matches!(jump, [0xEB, _, 0x90] | [0xE9, _, _]) {
            return Err(invalid("no x86 jump instruction"));
        }
        let oem_name = ascii_field(v.slice(3, 8).ok_or_else(|| invalid("truncated"))?);
        let bytes_per_sector = v.u16_le(11).ok_or_else(|| invalid("truncated"))?;
        if !bytes_per_sector.is_power_of_two() || !(512..=4096).contains(&bytes_per_sector) {
            return Err(FatError::InvalidBootSector(format!(
                "bytes per sector {bytes_per_sector} unsupported"
            )));
        }
        let sectors_per_cluster = v.u8(13).ok_or_else(|| invalid("truncated"))?;
        if sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two() {
            return Err(FatError::InvalidBootSector(format!(
                "sectors per cluster {sectors_per_cluster} invalid"
            )));
        }
        let reserved_sectors = v.u16_le(14).ok_or_else(|| invalid("truncated"))?;
        if reserved_sectors == 0 {
            return Err(invalid("reserved sector count is zero"));
        }
        let fat_count = v.u8(16).ok_or_else(|| invalid("truncated"))?;
        if !(1..=2).contains(&fat_count) {
            return Err(FatError::InvalidBootSector(format!(
                "FAT count {fat_count} invalid"
            )));
        }
        let root_entries = v.u16_le(17).ok_or_else(|| invalid("truncated"))?;
        let total16 = v.u16_le(19).ok_or_else(|| invalid("truncated"))?;
        let media = v.u8(21).ok_or_else(|| invalid("truncated"))?;
        if media != 0xF0 && media < 0xF8 {
            return Err(FatError::InvalidBootSector(format!(
                "media descriptor {media:#04x} invalid"
            )));
        }
        let spf16 = v.u16_le(22).ok_or_else(|| invalid("truncated"))?;
        let hidden_sectors = v.u32_le(28).ok_or_else(|| invalid("truncated"))?;
        let total32 = v.u32_le(32).ok_or_else(|| invalid("truncated"))?;
        let total_sectors = if total16 != 0 {
            u64::from(total16)
        } else {
            u64::from(total32)
        };
        if total_sectors == 0 {
            return Err(invalid("total sectors is zero"));
        }
        let is_fat32_layout = spf16 == 0;
        let sectors_per_fat = if is_fat32_layout {
            v.u32_le(36).ok_or_else(|| invalid("truncated"))?
        } else {
            u32::from(spf16)
        };
        if sectors_per_fat == 0 {
            return Err(invalid("sectors per FAT is zero"));
        }
        if v.u16_le(510) != Some(0xAA55) {
            return Err(invalid("missing 55 AA signature"));
        }

        let bps = u64::from(bytes_per_sector);
        let root_dir_bytes = arith::mul(u64::from(root_entries), 32)?;
        let root_dir_sectors = arith::div_ceil(root_dir_bytes, bps)?;
        let fat_region = arith::mul(u64::from(fat_count), u64::from(sectors_per_fat))?;
        let data_start_sector = arith::add(
            arith::add(u64::from(reserved_sectors), fat_region)?,
            root_dir_sectors,
        )?;
        if data_start_sector >= total_sectors {
            return Err(invalid("data region starts beyond the volume"));
        }
        let data_sectors = total_sectors - data_start_sector;
        let cluster_count64 = data_sectors / u64::from(sectors_per_cluster);
        let cluster_count =
            u32::try_from(cluster_count64).map_err(|_| invalid("too many clusters"))?;
        let variant = if is_fat32_layout {
            FatVariant::Fat32
        } else if cluster_count < 4085 {
            FatVariant::Fat12
        } else if cluster_count < 65_525 {
            FatVariant::Fat16
        } else {
            FatVariant::Fat32
        };
        if variant == FatVariant::Fat32 && !is_fat32_layout {
            return Err(invalid(
                "cluster count implies FAT32 but the BPB has a FAT16 layout",
            ));
        }
        if variant != FatVariant::Fat32 && root_entries == 0 {
            return Err(invalid("FAT12/16 volume without root directory entries"));
        }

        let (root_cluster, fs_info_sector, ext_flags, serial_off, label_off) = if is_fat32_layout {
            (
                v.u32_le(44).ok_or_else(|| invalid("truncated"))?,
                Some(v.u16_le(48).ok_or_else(|| invalid("truncated"))?),
                v.u16_le(40).ok_or_else(|| invalid("truncated"))?,
                67usize,
                71usize,
            )
        } else {
            (0, None, 0, 39usize, 43usize)
        };
        if is_fat32_layout && (root_cluster < 2 || root_cluster.saturating_sub(2) >= cluster_count)
        {
            return Err(FatError::InvalidBootSector(format!(
                "root cluster {root_cluster} outside the volume"
            )));
        }
        let volume_serial = v.u32_le(serial_off).unwrap_or(0);
        let volume_label = ascii_field(v.slice(label_off, 11).unwrap_or(b""))
            .trim_end()
            .to_owned();
        let type_label = ascii_field(v.slice(label_off + 11, 8).unwrap_or(b""))
            .trim_end()
            .to_owned();
        let cluster_size = u32::from(sectors_per_cluster) * u32::from(bytes_per_sector);
        let fat_offset = arith::mul(u64::from(reserved_sectors), bps)?;
        let fat_bytes = arith::mul(u64::from(sectors_per_fat), bps)?;
        let root_dir_offset = arith::add(fat_offset, arith::mul(fat_region, bps)?)?;
        let data_offset = arith::mul(data_start_sector, bps)?;
        Ok(Self {
            oem_name,
            bytes_per_sector,
            sectors_per_cluster,
            reserved_sectors,
            fat_count,
            root_entries,
            total_sectors,
            media,
            sectors_per_fat,
            hidden_sectors,
            root_cluster,
            fs_info_sector,
            ext_flags,
            volume_serial,
            volume_label,
            type_label,
            variant,
            cluster_count,
            cluster_size,
            fat_offset,
            fat_bytes,
            root_dir_offset,
            root_dir_bytes,
            data_offset,
        })
    }

    /// Whether `cluster` is a valid data cluster number.
    #[must_use]
    pub const fn is_valid_cluster(&self, cluster: u32) -> bool {
        cluster >= 2 && cluster - 2 < self.cluster_count
    }

    /// Volume byte offset of `cluster`.
    ///
    /// # Errors
    ///
    /// Returns [`FatError::InvalidChain`] for clusters outside the volume.
    pub fn cluster_offset(&self, cluster: u32) -> Result<u64, FatError> {
        if !self.is_valid_cluster(cluster) {
            return Err(FatError::InvalidChain(format!(
                "cluster {cluster} is outside the volume"
            )));
        }
        Ok(arith::add(
            self.data_offset,
            arith::mul(u64::from(cluster - 2), u64::from(self.cluster_size))?,
        )?)
    }

    /// Total volume size in bytes as declared.
    #[must_use]
    pub fn volume_bytes(&self) -> u64 {
        self.total_sectors
            .saturating_mul(u64::from(self.bytes_per_sector))
    }

    /// Byte offset of FAT number `index` (0-based).
    #[must_use]
    pub fn fat_offset_of(&self, index: u8) -> u64 {
        self.fat_offset
            .saturating_add(self.fat_bytes.saturating_mul(u64::from(index)))
    }

    /// Index of the active FAT (FAT32 may disable mirroring).
    #[must_use]
    pub const fn active_fat(&self) -> u8 {
        if self.variant as u8 == FatVariant::Fat32 as u8 && self.ext_flags & 0x80 != 0 {
            (self.ext_flags & 0x0F) as u8
        } else {
            0
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Synthetic boot sectors.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    pub fn fat32(total_sectors: u32, spc: u8, spf: u32) -> Vec<u8> {
        let mut s = vec![0u8; 512];
        s[0] = 0xEB;
        s[1] = 0x58;
        s[2] = 0x90;
        s[3..11].copy_from_slice(b"mkfs.fat");
        s[11..13].copy_from_slice(&512u16.to_le_bytes());
        s[13] = spc;
        s[14..16].copy_from_slice(&32u16.to_le_bytes());
        s[16] = 2;
        s[21] = 0xF8;
        s[32..36].copy_from_slice(&total_sectors.to_le_bytes());
        s[36..40].copy_from_slice(&spf.to_le_bytes());
        s[44..48].copy_from_slice(&2u32.to_le_bytes());
        s[48..50].copy_from_slice(&1u16.to_le_bytes());
        s[67..71].copy_from_slice(&0x1234_5678u32.to_le_bytes());
        s[71..82].copy_from_slice(b"TESTVOL    ");
        s[82..90].copy_from_slice(b"FAT32   ");
        s[510] = 0x55;
        s[511] = 0xAA;
        s
    }

    pub fn fat16(total_sectors: u16, spc: u8, spf: u16, root_entries: u16) -> Vec<u8> {
        let mut s = vec![0u8; 512];
        s[0] = 0xEB;
        s[1] = 0x3C;
        s[2] = 0x90;
        s[3..11].copy_from_slice(b"mkfs.fat");
        s[11..13].copy_from_slice(&512u16.to_le_bytes());
        s[13] = spc;
        s[14..16].copy_from_slice(&1u16.to_le_bytes());
        s[16] = 2;
        s[17..19].copy_from_slice(&root_entries.to_le_bytes());
        s[19..21].copy_from_slice(&total_sectors.to_le_bytes());
        s[21] = 0xF8;
        s[22..24].copy_from_slice(&spf.to_le_bytes());
        s[39..43].copy_from_slice(&0xABCD_EF01u32.to_le_bytes());
        s[43..54].copy_from_slice(b"SIXTEEN    ");
        s[54..62].copy_from_slice(b"FAT16   ");
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

    use super::testutil::{fat16, fat32};
    use super::*;

    #[test]
    fn parses_fat32_and_fat16_geometry() {
        let b = FatBootSector::parse(&fat32(131_072, 8, 128)).unwrap();
        assert_eq!(b.variant, FatVariant::Fat32);
        assert_eq!(b.cluster_size, 4096);
        assert_eq!(b.fat_offset, 32 * 512);
        assert_eq!(b.data_offset, (32 + 2 * 128) * 512);
        assert_eq!(b.cluster_count, (131_072 - 32 - 256) / 8);
        assert_eq!(b.volume_label, "TESTVOL");
        assert_eq!(b.cluster_offset(2).unwrap(), b.data_offset);
        assert!(b.cluster_offset(1).is_err());
        assert!(b.cluster_offset(b.cluster_count + 2).is_err());

        let b = FatBootSector::parse(&fat16(65_000, 4, 64, 512)).unwrap();
        assert_eq!(b.variant, FatVariant::Fat16);
        assert_eq!(b.root_dir_offset, (1 + 128) * 512);
        assert_eq!(b.root_dir_bytes, 512 * 32);
        assert_eq!(b.data_offset, (1 + 128 + 32) * 512);

        // Few clusters → FAT12 (8000 sectors at 2 per cluster ≈ 3975 clusters).
        let b = FatBootSector::parse(&fat16(8_000, 2, 8, 512)).unwrap();
        assert_eq!(b.variant, FatVariant::Fat12);
    }

    #[test]
    fn rejects_bad_sectors() {
        let mut s = fat32(131_072, 8, 128);
        s[13] = 3;
        assert!(FatBootSector::parse(&s).is_err());
        let mut s = fat32(131_072, 8, 128);
        s[510] = 0;
        assert!(FatBootSector::parse(&s).is_err());
        let mut s = fat32(131_072, 8, 128);
        s[44..48].copy_from_slice(&0u32.to_le_bytes());
        assert!(FatBootSector::parse(&s).is_err());
        assert!(FatBootSector::parse(&vec![0u8; 512]).is_err());
        assert!(FatBootSector::parse(&fat32(131_072, 8, 128)[..100]).is_err());
    }
}
