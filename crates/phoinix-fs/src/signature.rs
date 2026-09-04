//! Signature-only probes for filesystems without a native engine yet.
//!
//! These recognise FAT12/16/32, exFAT and the EXT family from their boot
//! sector or superblock so that `phoinix inspect` can label partitions. They
//! deliberately report moderate confidence: a matching signature is good
//! evidence, but nothing has been parsed beyond it.

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::FileSystemType;
use phoinix_core::bytes::ByteView;

use crate::{FileSystemProbe, FsError, ProbeEvidence, ProbeRegistry, ProbeResult};

/// Confidence assigned to a clean signature match.
pub const SIGNATURE_CONFIDENCE: u8 = 70;

/// Reads the first `len` bytes of `reader`, or fewer if the source is shorter.
fn read_prefix(reader: &dyn BlockReader, len: usize) -> Result<Vec<u8>, FsError> {
    let available = usize::try_from(reader.len().min(len as u64)).map_err(|_| FsError::Overflow)?;
    Ok(reader.read_vec(0, available)?)
}

/// Detects the FAT family (FAT12/16/32) from the BIOS Parameter Block.
#[derive(Debug, Default, Clone, Copy)]
pub struct FatProbe;

impl FileSystemProbe for FatProbe {
    fn filesystem(&self) -> FileSystemType {
        FileSystemType::Fat32
    }

    fn probe(&self, reader: &dyn BlockReader) -> Result<ProbeResult, FsError> {
        let sector = read_prefix(reader, 512)?;
        let view = ByteView::new(&sector);
        let Some(jump) = view.array::<3>(0) else {
            return Ok(ProbeResult::negative(
                FileSystemType::Fat32,
                "source shorter than a boot sector",
            ));
        };
        if !matches!(jump, [0xEB, _, 0x90] | [0xE9, _, _]) {
            return Ok(ProbeResult::negative(
                FileSystemType::Fat32,
                "no x86 jump instruction at offset 0",
            ));
        }
        let bps = view.u16_le(11).unwrap_or(0);
        let spc = view.u8(13).unwrap_or(0);
        let reserved = view.u16_le(14).unwrap_or(0);
        let fats = view.u8(16).unwrap_or(0);
        let root_entries = view.u16_le(17).unwrap_or(0);
        let total16 = view.u16_le(19).unwrap_or(0);
        let media = view.u8(21).unwrap_or(0);
        let spf16 = view.u16_le(22).unwrap_or(0);
        let total32 = view.u32_le(32).unwrap_or(0);
        let spf32 = view.u32_le(36).unwrap_or(0);
        let signature_ok = view.u16_le(510) == Some(0xAA55);

        let mut evidence = Vec::new();
        let bpb_ok = bps.is_power_of_two()
            && (512..=4096).contains(&bps)
            && spc.is_power_of_two()
            && reserved > 0
            && (1..=2).contains(&fats)
            && (media == 0xF0 || media >= 0xF8);
        if !bpb_ok {
            return Ok(ProbeResult::negative(
                FileSystemType::Fat32,
                "BIOS Parameter Block fields are invalid",
            ));
        }
        evidence.push(ProbeEvidence::supports(
            "BIOS Parameter Block fields are valid",
        ));
        evidence.push(if signature_ok {
            ProbeEvidence::supports("55 AA boot signature present")
        } else {
            ProbeEvidence::contradicts("55 AA boot signature missing")
        });

        let total = if total16 != 0 {
            u64::from(total16)
        } else {
            u64::from(total32)
        };
        let spf = if spf16 != 0 {
            u64::from(spf16)
        } else {
            u64::from(spf32)
        };
        if total == 0 || spf == 0 {
            return Ok(ProbeResult::negative(
                FileSystemType::Fat32,
                "zero total sectors or FAT size",
            ));
        }
        let root_sectors = (u64::from(root_entries) * 32).div_ceil(u64::from(bps));
        let data_start = u64::from(reserved) + u64::from(fats) * spf + root_sectors;
        let clusters = total.saturating_sub(data_start) / u64::from(spc);
        let (fs, label) = if spf16 == 0 && spf32 != 0 {
            (FileSystemType::Fat32, "FAT32")
        } else if clusters < 4085 {
            (FileSystemType::Fat12, "FAT12")
        } else if clusters < 65525 {
            (FileSystemType::Fat16, "FAT16")
        } else {
            (FileSystemType::Fat32, "FAT32")
        };
        evidence.push(ProbeEvidence::supports(format!(
            "{clusters} data clusters imply {label}"
        )));
        let fs_label_offset = if fs == FileSystemType::Fat32 { 82 } else { 54 };
        if let Some(label_bytes) = view.slice(fs_label_offset, 8)
            && label_bytes.starts_with(b"FAT")
        {
            evidence.push(ProbeEvidence::supports("filesystem type label present"));
        }
        let confidence = if signature_ok {
            SIGNATURE_CONFIDENCE
        } else {
            SIGNATURE_CONFIDENCE / 2
        };
        Ok(ProbeResult {
            filesystem: fs,
            confidence,
            evidence,
        })
    }
}

/// Detects exFAT from its boot region.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExFatProbe;

impl FileSystemProbe for ExFatProbe {
    fn filesystem(&self) -> FileSystemType {
        FileSystemType::ExFat
    }

    fn probe(&self, reader: &dyn BlockReader) -> Result<ProbeResult, FsError> {
        let sector = read_prefix(reader, 512)?;
        let view = ByteView::new(&sector);
        if view.slice(3, 8) != Some(b"EXFAT   ") {
            return Ok(ProbeResult::negative(
                FileSystemType::ExFat,
                "OEM name is not EXFAT",
            ));
        }
        let mut evidence = vec![ProbeEvidence::supports("OEM name is EXFAT")];
        let zero_region_ok = view
            .slice(11, 53)
            .is_some_and(|z| z.iter().all(|b| *b == 0));
        evidence.push(if zero_region_ok {
            ProbeEvidence::supports("must-be-zero region is zero")
        } else {
            ProbeEvidence::contradicts("must-be-zero region is not zero")
        });
        let bps_shift = view.u8(108).unwrap_or(0);
        let spc_shift = view.u8(109).unwrap_or(0);
        let shifts_ok = (9..=12).contains(&bps_shift) && spc_shift <= 25 - bps_shift;
        evidence.push(if shifts_ok {
            ProbeEvidence::supports("sector and cluster shifts are valid")
        } else {
            ProbeEvidence::contradicts("sector or cluster shift is invalid")
        });
        let signature_ok = view.u16_le(510) == Some(0xAA55);
        evidence.push(if signature_ok {
            ProbeEvidence::supports("55 AA boot signature present")
        } else {
            ProbeEvidence::contradicts("55 AA boot signature missing")
        });
        let supporting = evidence.iter().filter(|e| e.supports).count();
        let confidence = match supporting {
            4 => 85,
            3 => SIGNATURE_CONFIDENCE,
            _ => 30,
        };
        Ok(ProbeResult {
            filesystem: FileSystemType::ExFat,
            confidence,
            evidence,
        })
    }
}

/// Detects ext2/ext3/ext4 from the superblock magic at byte 1080.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExtProbe;

impl FileSystemProbe for ExtProbe {
    fn filesystem(&self) -> FileSystemType {
        FileSystemType::Ext
    }

    fn probe(&self, reader: &dyn BlockReader) -> Result<ProbeResult, FsError> {
        let prefix = read_prefix(reader, 2048)?;
        let view = ByteView::new(&prefix);
        let Some(sb) = view.sub(1024, 1024) else {
            return Ok(ProbeResult::negative(
                FileSystemType::Ext,
                "source shorter than a superblock",
            ));
        };
        if sb.u16_le(56) != Some(0xEF53) {
            return Ok(ProbeResult::negative(
                FileSystemType::Ext,
                "superblock magic 0xEF53 absent",
            ));
        }
        let mut evidence = vec![ProbeEvidence::supports("superblock magic 0xEF53 present")];
        let log_block = sb.u32_le(24).unwrap_or(u32::MAX);
        let inodes = sb.u32_le(0).unwrap_or(0);
        let blocks = sb.u32_le(4).unwrap_or(0);
        let geometry_ok = log_block <= 6 && inodes > 0 && blocks > 0;
        evidence.push(if geometry_ok {
            ProbeEvidence::supports("block size, inode and block counts are plausible")
        } else {
            ProbeEvidence::contradicts("block size, inode or block count is implausible")
        });
        let features_incompat = sb.u32_le(96).unwrap_or(0);
        let features_compat = sb.u32_le(92).unwrap_or(0);
        let flavour = if features_incompat & 0x0040 != 0 {
            "ext4 (extents)"
        } else if features_compat & 0x0004 != 0 {
            "ext3 (journal)"
        } else {
            "ext2"
        };
        evidence.push(ProbeEvidence::supports(format!(
            "feature flags indicate {flavour}"
        )));
        let confidence = if geometry_ok {
            SIGNATURE_CONFIDENCE
        } else {
            30
        };
        Ok(ProbeResult {
            filesystem: FileSystemType::Ext,
            confidence,
            evidence,
        })
    }
}

/// Registers every signature probe.
#[must_use]
pub fn register_all(registry: ProbeRegistry) -> ProbeRegistry {
    registry
        .with(Box::new(FatProbe))
        .with(Box::new(ExFatProbe))
        .with(Box::new(ExtProbe))
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
    use phoinix_block::MemoryReader;

    fn fat_boot(fat32: bool) -> Vec<u8> {
        let mut s = vec![0u8; 8192];
        s[0] = 0xEB;
        s[1] = 0x58;
        s[2] = 0x90;
        s[11..13].copy_from_slice(&512u16.to_le_bytes());
        s[13] = 8;
        s[14..16].copy_from_slice(&32u16.to_le_bytes());
        s[16] = 2;
        s[21] = 0xF8;
        if fat32 {
            s[32..36].copy_from_slice(&2_000_000u32.to_le_bytes());
            s[36..40].copy_from_slice(&1953u32.to_le_bytes());
            s[82..90].copy_from_slice(b"FAT32   ");
        } else {
            s[17..19].copy_from_slice(&512u16.to_le_bytes());
            s[19..21].copy_from_slice(&65_000u16.to_le_bytes());
            s[22..24].copy_from_slice(&32u16.to_le_bytes());
            s[54..62].copy_from_slice(b"FAT16   ");
        }
        s[510] = 0x55;
        s[511] = 0xAA;
        s
    }

    #[test]
    fn fat_variants() {
        let r = FatProbe.probe(&MemoryReader::new(fat_boot(true))).unwrap();
        assert_eq!(r.filesystem, FileSystemType::Fat32);
        assert!(r.is_positive());
        let r = FatProbe.probe(&MemoryReader::new(fat_boot(false))).unwrap();
        assert_eq!(r.filesystem, FileSystemType::Fat16);
        assert!(r.is_positive());
        let r = FatProbe.probe(&MemoryReader::zeroed(8192)).unwrap();
        assert!(!r.is_positive());
    }

    #[test]
    fn exfat_and_ext() {
        let mut s = vec![0u8; 8192];
        s[3..11].copy_from_slice(b"EXFAT   ");
        s[108] = 9;
        s[109] = 3;
        s[510] = 0x55;
        s[511] = 0xAA;
        let r = ExFatProbe.probe(&MemoryReader::new(s)).unwrap();
        assert_eq!(r.confidence, 85);

        let mut e = vec![0u8; 8192];
        e[1024..1028].copy_from_slice(&1000u32.to_le_bytes());
        e[1028..1032].copy_from_slice(&4000u32.to_le_bytes());
        e[1024 + 24..1024 + 28].copy_from_slice(&2u32.to_le_bytes());
        e[1080..1082].copy_from_slice(&0xEF53u16.to_le_bytes());
        e[1024 + 96..1024 + 100].copy_from_slice(&0x40u32.to_le_bytes());
        let r = ExtProbe.probe(&MemoryReader::new(e)).unwrap();
        assert!(r.is_positive());
        assert!(r.evidence.iter().any(|e| e.description.contains("ext4")));
    }

    #[test]
    fn registry_picks_best_and_tolerates_short_sources() {
        let registry = register_all(ProbeRegistry::new());
        assert_eq!(registry.len(), 3);
        let d = registry.detect(&MemoryReader::new(fat_boot(true)));
        assert_eq!(d.filesystem(), FileSystemType::Fat32);
        assert_eq!(d.results.len(), 3);
        let d = registry.detect(&MemoryReader::zeroed(100));
        assert_eq!(d.filesystem(), FileSystemType::Unknown);
    }
}
