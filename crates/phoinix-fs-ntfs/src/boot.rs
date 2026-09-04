//! NTFS boot sector (`$Boot`) parsing and validation.

use phoinix_core::arith;
use phoinix_core::bytes::{ByteView, ascii_field};
use serde::{Deserialize, Serialize};

use crate::NtfsError;

/// Smallest MFT/index record size PHOINIX accepts.
pub const MIN_RECORD_SIZE: u32 = 512;
/// Largest MFT/index record size PHOINIX accepts.
pub const MAX_RECORD_SIZE: u32 = 64 * 1024;
/// Largest cluster size PHOINIX accepts (Windows allows up to 2 MiB).
pub const MAX_CLUSTER_SIZE: u32 = 2 * 1024 * 1024;

/// Parsed and validated NTFS boot sector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NtfsBootSector {
    /// OEM identifier (normally `NTFS    `).
    pub oem_id: String,
    /// Bytes per sector.
    pub bytes_per_sector: u16,
    /// Sectors per cluster (decoded; may exceed 255 for large clusters).
    pub sectors_per_cluster: u32,
    /// Cluster size in bytes.
    pub cluster_size: u32,
    /// Media descriptor byte.
    pub media_descriptor: u8,
    /// Sectors before this volume on the disk (informational).
    pub hidden_sectors: u32,
    /// Total sectors in the volume (the final sector holds the backup boot
    /// sector and is not counted by Windows).
    pub total_sectors: u64,
    /// Logical cluster number of `$MFT`.
    pub mft_lcn: u64,
    /// Logical cluster number of `$MFTMirr`.
    pub mft_mirror_lcn: u64,
    /// Size of one MFT FILE record in bytes.
    pub mft_record_size: u32,
    /// Size of one index record in bytes.
    pub index_record_size: u32,
    /// Volume serial number.
    pub volume_serial: u64,
    /// Boot-sector checksum field (unused by Windows).
    pub checksum: u32,
}

impl NtfsBootSector {
    /// Parses the first 512 bytes of a volume.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::InvalidBootSector`] describing the first failed
    /// check.
    pub fn parse(sector: &[u8]) -> Result<Self, NtfsError> {
        let view = ByteView::new(sector);
        let invalid = |what: &str| NtfsError::InvalidBootSector(what.to_owned());
        let field = |name: &'static str, v: Option<u64>| {
            v.ok_or_else(|| invalid(&format!("truncated before {name}")))
        };

        let oem_bytes = view
            .slice(3, 8)
            .ok_or_else(|| invalid("truncated before OEM ID"))?;
        let oem_id = ascii_field(oem_bytes);
        if oem_bytes != b"NTFS    " {
            return Err(NtfsError::InvalidBootSector(format!(
                "OEM ID is {oem_id:?}, not \"NTFS    \""
            )));
        }
        let bytes_per_sector =
            u16::try_from(field("bytes per sector", view.u16_le(11).map(u64::from))?)
                .map_err(|_| invalid("bytes per sector"))?;
        if !bytes_per_sector.is_power_of_two() || !(256..=4096).contains(&bytes_per_sector) {
            return Err(NtfsError::InvalidBootSector(format!(
                "bytes per sector {bytes_per_sector} is not a supported power of two"
            )));
        }
        let spc_raw = view
            .u8(13)
            .ok_or_else(|| invalid("truncated before sectors per cluster"))?;
        // Values >= 0x80 encode 2^(256 - value) sectors (used for clusters larger than 64 KiB).
        let sectors_per_cluster: u32 = if spc_raw >= 0x80 {
            1u32.checked_shl(u32::from(256u16.wrapping_sub(u16::from(spc_raw))))
                .unwrap_or(0)
        } else {
            u32::from(spc_raw)
        };
        if sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two() {
            return Err(NtfsError::InvalidBootSector(format!(
                "sectors per cluster {spc_raw:#04x} is invalid"
            )));
        }
        let cluster_size = sectors_per_cluster
            .checked_mul(u32::from(bytes_per_sector))
            .filter(|c| *c <= MAX_CLUSTER_SIZE)
            .ok_or_else(|| invalid("cluster size is unreasonable"))?;

        // Fields that must be zero on NTFS (reserved sectors, FATs, root entries, total sectors 16, sectors per FAT).
        let reserved_ok = view.u16_le(14) == Some(0)
            && view.u8(16) == Some(0)
            && view.u16_le(17) == Some(0)
            && view.u16_le(19) == Some(0)
            && view.u16_le(22) == Some(0)
            && view.u32_le(32) == Some(0);
        if !reserved_ok {
            return Err(invalid("FAT-only BPB fields are not zero"));
        }
        let media_descriptor = view.u8(21).ok_or_else(|| invalid("truncated"))?;
        let hidden_sectors =
            u32::try_from(field("hidden sectors", view.u32_le(28).map(u64::from))?).unwrap_or(0);
        let total_sectors = field("total sectors", view.u64_le(40))?;
        if total_sectors == 0 {
            return Err(invalid("total sectors is zero"));
        }
        let mft_lcn = field("$MFT LCN", view.u64_le(48))?;
        let mft_mirror_lcn = field("$MFTMirr LCN", view.u64_le(56))?;
        let total_clusters = total_sectors / u64::from(sectors_per_cluster);
        if mft_lcn >= total_clusters {
            return Err(NtfsError::InvalidBootSector(format!(
                "$MFT LCN {mft_lcn} is outside the volume ({total_clusters} clusters)"
            )));
        }
        if mft_mirror_lcn >= total_clusters {
            return Err(NtfsError::InvalidBootSector(format!(
                "$MFTMirr LCN {mft_mirror_lcn} is outside the volume ({total_clusters} clusters)"
            )));
        }
        let mft_record_size = decode_record_size(
            view.i8(64).ok_or_else(|| invalid("truncated"))?,
            cluster_size,
        )
        .ok_or_else(|| invalid("MFT record size is invalid"))?;
        let index_record_size = decode_record_size(
            view.i8(68).ok_or_else(|| invalid("truncated"))?,
            cluster_size,
        )
        .ok_or_else(|| invalid("index record size is invalid"))?;
        let volume_serial = field("volume serial", view.u64_le(72))?;
        let checksum =
            u32::try_from(field("checksum", view.u32_le(80).map(u64::from))?).unwrap_or(0);
        if view.u16_le(510) != Some(0xAA55) {
            return Err(invalid("missing 55 AA boot signature"));
        }
        Ok(Self {
            oem_id,
            bytes_per_sector,
            sectors_per_cluster,
            cluster_size,
            media_descriptor,
            hidden_sectors,
            total_sectors,
            mft_lcn,
            mft_mirror_lcn,
            mft_record_size,
            index_record_size,
            volume_serial,
            checksum,
        })
    }

    /// Total volume size in bytes as declared by the boot sector.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::Overflow`] if the product overflows.
    pub fn volume_bytes(&self) -> Result<u64, NtfsError> {
        Ok(arith::mul(
            self.total_sectors,
            u64::from(self.bytes_per_sector),
        )?)
    }

    /// Number of whole clusters in the volume.
    #[must_use]
    pub const fn total_clusters(&self) -> u64 {
        self.total_sectors / (self.sectors_per_cluster as u64)
    }

    /// Converts a logical cluster number to a volume byte offset.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::Overflow`] if the product overflows.
    pub fn lcn_to_offset(&self, lcn: u64) -> Result<u64, NtfsError> {
        Ok(arith::mul(lcn, u64::from(self.cluster_size))?)
    }

    /// Byte offset of `$MFT`'s first record.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::Overflow`] if the product overflows.
    pub fn mft_offset(&self) -> Result<u64, NtfsError> {
        self.lcn_to_offset(self.mft_lcn)
    }

    /// Whether the declared volume fits inside a source of `source_len` bytes.
    ///
    /// The boot sector counts one fewer sector than the partition usually
    /// holds (the backup boot sector), so the check uses `total_sectors`.
    #[must_use]
    pub fn fits_in(&self, source_len: u64) -> bool {
        self.volume_bytes().is_ok_and(|bytes| bytes <= source_len)
    }
}

/// Decodes the signed "clusters per record" byte.
///
/// Positive values count clusters; negative values are `log2` of the size in
/// bytes (`-10` → 1024).
fn decode_record_size(raw: i8, cluster_size: u32) -> Option<u32> {
    let size = if raw >= 0 {
        u32::try_from(raw).ok()?.checked_mul(cluster_size)?
    } else {
        let shift = u32::try_from(-i32::from(raw)).ok()?;
        1u32.checked_shl(shift)?
    };
    if (MIN_RECORD_SIZE..=MAX_RECORD_SIZE).contains(&size) && size.is_power_of_two() {
        Some(size)
    } else {
        None
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builder for synthetic boot sectors.

    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    /// Fields of a synthetic boot sector.
    pub struct BootSpec {
        pub bytes_per_sector: u16,
        pub sectors_per_cluster: u8,
        pub total_sectors: u64,
        pub mft_lcn: u64,
        pub mft_mirror_lcn: u64,
        pub record_size_raw: i8,
        pub index_size_raw: i8,
    }

    impl Default for BootSpec {
        fn default() -> Self {
            Self {
                bytes_per_sector: 512,
                sectors_per_cluster: 8,
                total_sectors: 131_071,
                mft_lcn: 4,
                mft_mirror_lcn: 8191,
                record_size_raw: -10,
                index_size_raw: 1,
            }
        }
    }

    /// Builds a 512-byte boot sector.
    pub fn build(spec: &BootSpec) -> Vec<u8> {
        let mut s = vec![0u8; 512];
        s[0] = 0xEB;
        s[1] = 0x52;
        s[2] = 0x90;
        s[3..11].copy_from_slice(b"NTFS    ");
        s[11..13].copy_from_slice(&spec.bytes_per_sector.to_le_bytes());
        s[13] = spec.sectors_per_cluster;
        s[21] = 0xF8;
        s[24..26].copy_from_slice(&63u16.to_le_bytes());
        s[26..28].copy_from_slice(&255u16.to_le_bytes());
        s[36] = 0x80;
        s[38] = 0x80;
        s[40..48].copy_from_slice(&spec.total_sectors.to_le_bytes());
        s[48..56].copy_from_slice(&spec.mft_lcn.to_le_bytes());
        s[56..64].copy_from_slice(&spec.mft_mirror_lcn.to_le_bytes());
        s[64] = spec.record_size_raw.to_le_bytes()[0];
        s[68] = spec.index_size_raw.to_le_bytes()[0];
        s[72..80].copy_from_slice(&0x1234_5678_9ABC_DEF0u64.to_le_bytes());
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

    use super::testutil::{BootSpec, build};
    use super::*;

    #[test]
    fn parses_valid_sector() {
        let b = NtfsBootSector::parse(&build(&BootSpec::default())).unwrap();
        assert_eq!(b.bytes_per_sector, 512);
        assert_eq!(b.sectors_per_cluster, 8);
        assert_eq!(b.cluster_size, 4096);
        assert_eq!(b.mft_record_size, 1024);
        assert_eq!(b.index_record_size, 4096);
        assert_eq!(b.mft_lcn, 4);
        assert_eq!(b.mft_offset().unwrap(), 16384);
        assert_eq!(b.total_clusters(), 16383);
        assert_eq!(b.volume_serial, 0x1234_5678_9ABC_DEF0);
        assert!(b.fits_in(131_072 * 512));
        assert!(!b.fits_in(131_070 * 512));
    }

    #[test]
    fn positive_record_size_and_large_clusters() {
        let spec = BootSpec {
            record_size_raw: 1,
            index_size_raw: -12,
            sectors_per_cluster: 2,
            ..BootSpec::default()
        };
        let b = NtfsBootSector::parse(&build(&spec)).unwrap();
        assert_eq!(b.cluster_size, 1024);
        assert_eq!(b.mft_record_size, 1024);
        assert_eq!(b.index_record_size, 4096);

        // 0xF1 => 2^(256-241) = 2^15 sectors = 16 MiB at 512 B/s: too large.
        let spec = BootSpec {
            sectors_per_cluster: 0xF1,
            ..BootSpec::default()
        };
        assert!(NtfsBootSector::parse(&build(&spec)).is_err());
        // 0xF8 => 2^8 = 256 sectors = 128 KiB clusters: allowed.
        let spec = BootSpec {
            sectors_per_cluster: 0xF8,
            total_sectors: 1 << 20,
            mft_lcn: 4,
            mft_mirror_lcn: 5,
            index_size_raw: -12,
            ..BootSpec::default()
        };
        assert_eq!(
            NtfsBootSector::parse(&build(&spec)).unwrap().cluster_size,
            131_072
        );
    }

    #[test]
    fn rejects_bad_fields() {
        type Mutation = Box<dyn Fn(&mut Vec<u8>)>;
        let cases: Vec<(&str, Mutation)> = vec![
            ("oem", Box::new(|s| s[3..11].copy_from_slice(b"MSDOS5.0"))),
            (
                "bytes per sector",
                Box::new(|s| s[11..13].copy_from_slice(&513u16.to_le_bytes())),
            ),
            ("sectors per cluster", Box::new(|s| s[13] = 3)),
            ("record size", Box::new(|s| s[64] = 0x7F)),
            (
                "mft outside",
                Box::new(|s| s[48..56].copy_from_slice(&u64::MAX.to_le_bytes())),
            ),
            ("signature", Box::new(|s| s[510] = 0)),
            ("reserved sectors", Box::new(|s| s[14] = 1)),
            (
                "total zero",
                Box::new(|s| s[40..48].copy_from_slice(&0u64.to_le_bytes())),
            ),
        ];
        for (name, mutate) in cases {
            let mut s = build(&BootSpec::default());
            mutate(&mut s);
            assert!(
                matches!(
                    NtfsBootSector::parse(&s),
                    Err(NtfsError::InvalidBootSector(_))
                ),
                "{name} should be rejected"
            );
        }
        assert!(NtfsBootSector::parse(&build(&BootSpec::default())[..100]).is_err());
    }
}
