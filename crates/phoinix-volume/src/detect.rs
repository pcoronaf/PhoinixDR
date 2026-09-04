//! Partition-scheme detection.

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::bytes::ByteView;

use crate::gpt::scan_gpt;
use crate::mbr::{MbrSector, scan_mbr};
use crate::{PartitionScheme, PartitionTable, VolumeDiagnostic, VolumeError};

/// Heuristically decides whether a sector is a filesystem boot sector (NTFS,
/// FAT, exFAT) rather than a partition table.
///
/// Such sectors also carry `55 AA` and hold boot code where MBR entries would
/// be, so they must be recognised before entries are interpreted.
#[must_use]
pub fn looks_like_filesystem_boot_sector(sector: &[u8]) -> bool {
    let view = ByteView::new(sector);
    let Some(jump) = view.array::<3>(0) else {
        return false;
    };
    let Some(oem) = view.slice(3, 8) else {
        return false;
    };
    let jump_ok = matches!(jump, [0xEB, _, 0x90] | [0xE9, _, _]);
    if !jump_ok {
        return false;
    }
    if oem == b"NTFS    " || oem == b"EXFAT   " {
        return true;
    }
    // FAT BPB: bytes per sector power of two 512..=4096 and sectors per
    // cluster power of two, plus a media descriptor of 0xF0 or 0xF8..=0xFF.
    let bps = view.u16_le(11).unwrap_or(0);
    let spc = view.u8(13).unwrap_or(0);
    let media = view.u8(21).unwrap_or(0);
    let reserved = view.u16_le(14).unwrap_or(0);
    let fats = view.u8(16).unwrap_or(0);
    bps.is_power_of_two()
        && (512..=4096).contains(&bps)
        && spc.is_power_of_two()
        && (media == 0xF0 || media >= 0xF8)
        && reserved > 0
        && (1..=2).contains(&fats)
}

/// Reads and validates the partition table of `reader`.
///
/// Never fails on malformed tables: they yield a table with diagnostics and
/// whatever partitions could be trusted.
///
/// # Errors
///
/// Returns [`VolumeError::SourceTooSmall`] if there is not even one sector,
/// or an I/O error.
pub fn read_partition_table(reader: &dyn BlockReader) -> Result<PartitionTable, VolumeError> {
    let sector_size = reader.geometry().logical_sector_size;
    if reader.len() < u64::from(sector_size) {
        return Err(VolumeError::SourceTooSmall { len: reader.len() });
    }
    let total_sectors = reader.len() / u64::from(sector_size);
    let lba0 = reader.read_sector(0)?;
    let mut diagnostics = Vec::new();

    if looks_like_filesystem_boot_sector(&lba0) {
        diagnostics.push(VolumeDiagnostic::FilesystemBootSectorAtLba0);
        tracing::debug!("LBA 0 is a filesystem boot sector; treating source as a bare volume");
        return Ok(PartitionTable::none(sector_size, diagnostics));
    }

    let mbr = MbrSector::parse(&lba0);
    let protective = mbr
        .as_ref()
        .is_some_and(|m| m.has_signature && m.has_protective_entry());

    // GPT is authoritative whenever a valid header exists.
    if let Some(gpt) = scan_gpt(reader, &mut diagnostics)? {
        if !protective {
            diagnostics.push(VolumeDiagnostic::ProtectiveMbrMissing);
        }
        let mut table = PartitionTable {
            scheme: PartitionScheme::Gpt,
            sector_size,
            partitions: gpt.partitions,
            diagnostics,
            disk_guid: Some(gpt.header.disk_guid),
            mbr_disk_signature: mbr.as_ref().map(|m| m.disk_signature),
        };
        table.validate_against(reader.len());
        tracing::debug!(
            partitions = table.partitions.len(),
            backup = gpt.used_backup,
            "GPT read"
        );
        return Ok(table);
    }
    if protective {
        diagnostics.push(VolumeDiagnostic::ProtectiveMbrWithoutGpt);
    }

    let Some(mbr) = mbr else {
        return Ok(PartitionTable::none(sector_size, diagnostics));
    };
    if !mbr.has_signature {
        diagnostics.push(VolumeDiagnostic::InvalidMbrSignature);
        let mut table = PartitionTable::none(sector_size, diagnostics);
        table.scheme = if lba0.iter().all(|b| *b == 0) {
            PartitionScheme::None
        } else {
            PartitionScheme::Unknown
        };
        return Ok(table);
    }
    if mbr.is_blank() {
        let mut table = PartitionTable::none(sector_size, diagnostics);
        table.mbr_disk_signature = Some(mbr.disk_signature);
        return Ok(table);
    }
    if !mbr.entries_plausible(total_sectors) && !protective {
        diagnostics.push(VolumeDiagnostic::ImplausibleMbrEntries);
        let mut table = PartitionTable::none(sector_size, diagnostics);
        table.scheme = PartitionScheme::Unknown;
        return Ok(table);
    }

    let scan = scan_mbr(reader, &mbr)?;
    diagnostics.extend(scan.diagnostics);
    let mut table = PartitionTable {
        scheme: PartitionScheme::Mbr,
        sector_size,
        partitions: scan.partitions,
        diagnostics,
        disk_guid: None,
        mbr_disk_signature: Some(mbr.disk_signature),
    };
    table.validate_against(reader.len());
    tracing::debug!(partitions = table.partitions.len(), "MBR read");
    Ok(table)
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
    use crate::gpt::testutil::{Layout, Part, build};
    use crate::mbr::testutil::sector;
    use crate::model::gpt_types;
    use crate::{PartitionConfidence, PartitionType};
    use phoinix_block::MemoryReader;
    use std::sync::Arc;

    #[test]
    fn detects_mbr() {
        let mut d = vec![0u8; 16384 * 512];
        d[..512].copy_from_slice(&sector(
            &[(0x80, 0x07, 2048, 4096), (0, 0x83, 6144, 8192)],
            true,
        ));
        let t = read_partition_table(&MemoryReader::new(d)).unwrap();
        assert_eq!(t.scheme, PartitionScheme::Mbr);
        assert_eq!(t.partitions.len(), 2);
        assert!(t.diagnostics.is_empty());
    }

    #[test]
    fn detects_gpt_and_reports_missing_protective_mbr() {
        let mut layout = Layout::new(
            8192,
            vec![Part {
                type_guid: gpt_types::BASIC_DATA,
                first: 34,
                last: 8000,
                name: "data",
                attributes: 0,
            }],
        );
        let t = read_partition_table(&MemoryReader::new(build(&layout))).unwrap();
        assert_eq!(t.scheme, PartitionScheme::Gpt);
        assert!(t.diagnostics.is_empty(), "{:?}", t.diagnostics);
        assert!(t.disk_guid.is_some());

        layout.protective_mbr = false;
        let t = read_partition_table(&MemoryReader::new(build(&layout))).unwrap();
        assert_eq!(t.scheme, PartitionScheme::Gpt);
        assert!(
            t.diagnostics
                .contains(&VolumeDiagnostic::ProtectiveMbrMissing)
        );
    }

    #[test]
    fn protective_mbr_without_gpt() {
        let mut d = vec![0u8; 4096 * 512];
        d[..512].copy_from_slice(&sector(&[(0, 0xEE, 1, 4095)], true));
        let t = read_partition_table(&MemoryReader::new(d)).unwrap();
        assert!(
            t.diagnostics
                .contains(&VolumeDiagnostic::ProtectiveMbrWithoutGpt)
        );
        // The protective entry is still reported as an MBR partition so that
        // the user sees something; it is filtered from `volumes()`.
        assert_eq!(t.scheme, PartitionScheme::Mbr);
        assert_eq!(t.volumes().count(), 0);
    }

    #[test]
    fn missing_signature_and_blank_disk() {
        let t = read_partition_table(&MemoryReader::zeroed(4096 * 512)).unwrap();
        assert_eq!(t.scheme, PartitionScheme::None);
        assert!(
            t.diagnostics
                .contains(&VolumeDiagnostic::InvalidMbrSignature)
        );

        let mut d = vec![0x5Au8; 4096 * 512];
        d[510] = 0;
        let t = read_partition_table(&MemoryReader::new(d)).unwrap();
        assert_eq!(t.scheme, PartitionScheme::Unknown);

        let mut d = vec![0u8; 4096 * 512];
        d[..512].copy_from_slice(&sector(&[], true));
        let t = read_partition_table(&MemoryReader::new(d)).unwrap();
        assert_eq!(t.scheme, PartitionScheme::None);
        assert!(t.diagnostics.is_empty());
    }

    #[test]
    fn ntfs_boot_sector_is_not_a_partition_table() {
        let mut d = vec![0u8; 4096 * 512];
        d[0] = 0xEB;
        d[1] = 0x52;
        d[2] = 0x90;
        d[3..11].copy_from_slice(b"NTFS    ");
        // Boot code garbage where entries live.
        for (i, b) in d[446..510].iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(37);
        }
        d[510] = 0x55;
        d[511] = 0xAA;
        let t = read_partition_table(&MemoryReader::new(d)).unwrap();
        assert_eq!(t.scheme, PartitionScheme::None);
        assert!(
            t.diagnostics
                .contains(&VolumeDiagnostic::FilesystemBootSectorAtLba0)
        );
        assert!(looks_like_filesystem_boot_sector(&{
            let mut s = vec![0u8; 512];
            s[0] = 0xEB;
            s[2] = 0x90;
            s[11] = 0x00;
            s[12] = 0x02;
            s[13] = 8;
            s[14] = 32;
            s[16] = 2;
            s[21] = 0xF8;
            s
        }));
    }

    #[test]
    fn partition_beyond_device_is_flagged_and_clamped() {
        let mut d = vec![0u8; 4096 * 512];
        d[..512].copy_from_slice(&sector(&[(0, 0x07, 2048, 4096)], true));
        let r: Arc<dyn BlockReader> = Arc::new(MemoryReader::new(d));
        let t = read_partition_table(&*r).unwrap();
        assert!(
            t.diagnostics
                .contains(&VolumeDiagnostic::PartitionOutsideDevice { index: 1 })
        );
        assert_eq!(t.partitions[0].confidence, PartitionConfidence::Low);
        let view = t.partitions[0].open(r.clone()).unwrap();
        assert_eq!(view.len(), 2048 * 512);
        // A partition starting past the end cannot be opened.
        let mut far = t.partitions[0].clone();
        far.start_offset = 5000 * 512;
        assert!(far.open(r).is_err());
    }

    #[test]
    fn overlapping_partitions_are_flagged() {
        let mut d = vec![0u8; 16384 * 512];
        d[..512].copy_from_slice(&sector(
            &[(0, 0x07, 2048, 4096), (0, 0x83, 4096, 4096)],
            true,
        ));
        let t = read_partition_table(&MemoryReader::new(d)).unwrap();
        assert!(
            t.diagnostics
                .contains(&VolumeDiagnostic::OverlappingPartitions {
                    first: 1,
                    second: 2
                })
        );
        assert!(
            t.partitions
                .iter()
                .all(|p| p.confidence == PartitionConfidence::Medium)
        );
    }

    #[test]
    fn overlapping_gpt_partitions_are_flagged() {
        let layout = Layout::new(
            8192,
            vec![
                Part {
                    type_guid: gpt_types::BASIC_DATA,
                    first: 34,
                    last: 4000,
                    name: "a",
                    attributes: 0,
                },
                Part {
                    type_guid: gpt_types::BASIC_DATA,
                    first: 3000,
                    last: 8000,
                    name: "b",
                    attributes: 0,
                },
            ],
        );
        let t = read_partition_table(&MemoryReader::new(build(&layout))).unwrap();
        assert!(
            t.diagnostics
                .contains(&VolumeDiagnostic::OverlappingPartitions {
                    first: 1,
                    second: 2
                })
        );
    }

    #[test]
    fn extended_container_overlap_is_not_flagged() {
        let mut d = vec![0u8; 16384 * 512];
        d[..512].copy_from_slice(&sector(&[(0, 0x05, 4096, 12288)], true));
        let e1 = 4096 * 512;
        d[e1..e1 + 512].copy_from_slice(&sector(&[(0, 0x83, 2048, 2048)], true));
        let t = read_partition_table(&MemoryReader::new(d)).unwrap();
        assert_eq!(t.partitions.len(), 2);
        assert!(t.diagnostics.is_empty(), "{:?}", t.diagnostics);
        assert_eq!(t.volumes().count(), 1);
        assert_eq!(t.partitions[1].partition_type, PartitionType::Mbr(0x83));
    }

    #[test]
    fn source_too_small() {
        assert!(matches!(
            read_partition_table(&MemoryReader::zeroed(100)),
            Err(VolumeError::SourceTooSmall { .. })
        ));
    }

    #[test]
    fn table_serialises() {
        let mut d = vec![0u8; 16384 * 512];
        d[..512].copy_from_slice(&sector(&[(0x80, 0x07, 2048, 4096)], true));
        let t = read_partition_table(&MemoryReader::new(d)).unwrap();
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains("\"scheme\":\"mbr\""));
    }
}
