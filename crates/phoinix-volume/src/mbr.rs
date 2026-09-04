//! Master Boot Record and extended-partition (EBR chain) parsing.

use std::collections::HashSet;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::arith;
use phoinix_core::bytes::ByteView;

use crate::{
    Partition, PartitionConfidence, PartitionFlags, PartitionType, VolumeDiagnostic, VolumeError,
};

/// Offset of the first partition entry inside LBA 0.
pub const ENTRY_TABLE_OFFSET: usize = 446;
/// Size of one partition entry.
pub const ENTRY_SIZE: usize = 16;
/// Offset of the `55 AA` signature.
pub const SIGNATURE_OFFSET: usize = 510;
/// Offset of the 32-bit disk signature.
pub const DISK_SIGNATURE_OFFSET: usize = 440;
/// Maximum number of EBRs followed in one extended partition.
pub const MAX_EBR_CHAIN: usize = 4096;

/// One of the four raw MBR partition entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MbrEntry {
    /// Status byte (`0x80` = active).
    pub status: u8,
    /// Partition type byte.
    pub partition_type: u8,
    /// CHS address of the first sector (parsed, never trusted).
    pub chs_first: [u8; 3],
    /// CHS address of the last sector (parsed, never trusted).
    pub chs_last: [u8; 3],
    /// First LBA, relative to the containing table's base.
    pub first_lba: u32,
    /// Sector count.
    pub sectors: u32,
}

impl MbrEntry {
    /// Whether the entry is unused.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.partition_type == 0 && self.sectors == 0
    }

    /// Whether the status byte is one of the two valid values.
    #[must_use]
    pub const fn has_valid_status(&self) -> bool {
        matches!(self.status, 0x00 | 0x80)
    }

    /// Whether this entry describes an extended container.
    #[must_use]
    pub const fn is_extended(&self) -> bool {
        matches!(self.partition_type, 0x05 | 0x0F | 0x85)
    }
}

/// A parsed boot-sector-sized table (an MBR or an EBR).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MbrSector {
    /// Whether the `55 AA` signature was present.
    pub has_signature: bool,
    /// 32-bit disk signature at offset 440.
    pub disk_signature: u32,
    /// The four entries.
    pub entries: [MbrEntry; 4],
}

impl MbrSector {
    /// Parses the first 512 bytes of a sector.
    ///
    /// Returns [`None`] if fewer than 512 bytes are supplied.
    #[must_use]
    pub fn parse(sector: &[u8]) -> Option<Self> {
        let view = ByteView::new(sector);
        let has_signature =
            view.u8(SIGNATURE_OFFSET)? == 0x55 && view.u8(SIGNATURE_OFFSET + 1)? == 0xAA;
        let disk_signature = view.u32_le(DISK_SIGNATURE_OFFSET)?;
        let mut entries = [MbrEntry {
            status: 0,
            partition_type: 0,
            chs_first: [0; 3],
            chs_last: [0; 3],
            first_lba: 0,
            sectors: 0,
        }; 4];
        for (i, entry) in entries.iter_mut().enumerate() {
            let base = ENTRY_TABLE_OFFSET + i * ENTRY_SIZE;
            *entry = MbrEntry {
                status: view.u8(base)?,
                chs_first: view.array::<3>(base + 1)?,
                partition_type: view.u8(base + 4)?,
                chs_last: view.array::<3>(base + 5)?,
                first_lba: view.u32_le(base + 8)?,
                sectors: view.u32_le(base + 12)?,
            };
        }
        Some(Self {
            has_signature,
            disk_signature,
            entries,
        })
    }

    /// Whether any entry has the GPT protective type.
    #[must_use]
    pub fn has_protective_entry(&self) -> bool {
        self.entries.iter().any(|e| e.partition_type == 0xEE)
    }

    /// Whether all entries are unused.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.entries.iter().all(MbrEntry::is_empty)
    }

    /// Whether the non-empty entries look like real partition entries.
    ///
    /// Boot code in a filesystem boot sector also occupies bytes 446..510,
    /// so this guards against interpreting code as a table.
    #[must_use]
    pub fn entries_plausible(&self, total_sectors: u64) -> bool {
        let mut any = false;
        for e in &self.entries {
            if e.is_empty() {
                continue;
            }
            any = true;
            if !e.has_valid_status() || e.partition_type == 0 || e.sectors == 0 {
                return false;
            }
            // Type 0 with a size, or a start at LBA 0 (inside the MBR), is not a real partition.
            if e.first_lba == 0 {
                return false;
            }
            // Permit entries slightly beyond the end (truncated images) but
            // not wildly so.
            if u64::from(e.first_lba) >= total_sectors.saturating_mul(2).max(1) {
                return false;
            }
        }
        any
    }
}

/// Result of walking an MBR and its extended partitions.
#[derive(Debug, Default)]
pub struct MbrScan {
    /// Partitions found (primary 1–4, logical from 5).
    pub partitions: Vec<Partition>,
    /// Findings.
    pub diagnostics: Vec<VolumeDiagnostic>,
}

/// Converts the primary entries of an MBR into partitions and follows every
/// extended container.
///
/// # Errors
///
/// Returns [`VolumeError`] only for I/O failures on the primary sector; EBR
/// read failures become diagnostics.
pub fn scan_mbr(reader: &dyn BlockReader, mbr: &MbrSector) -> Result<MbrScan, VolumeError> {
    let sector_size = reader.geometry().logical_sector_size;
    let total_sectors = reader.len() / u64::from(sector_size);
    let mut scan = MbrScan::default();
    let mut next_logical_index: u32 = 5;

    for (i, entry) in mbr.entries.iter().enumerate() {
        let index = u32::try_from(i).unwrap_or(0) + 1;
        if entry.is_empty() {
            continue;
        }
        if entry.sectors == 0 || entry.partition_type == 0 {
            scan.diagnostics
                .push(VolumeDiagnostic::ZeroLengthPartition { index });
            continue;
        }
        let start = u64::from(entry.first_lba);
        let end = arith::add(start, u64::from(entry.sectors))? - 1;
        let mut partition = Partition::from_lba(
            index,
            start,
            end,
            sector_size,
            PartitionType::Mbr(entry.partition_type),
        )?;
        if entry.status == 0x80 {
            partition.flags |= PartitionFlags::BOOTABLE;
        }
        if !entry.has_valid_status() {
            scan.diagnostics
                .push(VolumeDiagnostic::InvalidMbrEntryStatus { index });
            partition.confidence = PartitionConfidence::Medium;
        }
        scan.partitions.push(partition);

        if entry.is_extended() {
            walk_extended(
                reader,
                start,
                u64::from(entry.sectors),
                total_sectors,
                &mut next_logical_index,
                &mut scan,
            );
        }
    }
    Ok(scan)
}

/// Follows the EBR chain of one extended container.
fn walk_extended(
    reader: &dyn BlockReader,
    container_start: u64,
    container_sectors: u64,
    total_sectors: u64,
    next_index: &mut u32,
    scan: &mut MbrScan,
) {
    let sector_size = reader.geometry().logical_sector_size;
    let mut visited: HashSet<u64> = HashSet::new();
    let mut current = container_start;
    let container_end = container_start.saturating_add(container_sectors);

    for depth in 0..=MAX_EBR_CHAIN {
        if depth == MAX_EBR_CHAIN {
            scan.diagnostics
                .push(VolumeDiagnostic::ExtendedPartitionTooDeep);
            return;
        }
        if !visited.insert(current) {
            scan.diagnostics
                .push(VolumeDiagnostic::ExtendedPartitionLoop);
            return;
        }
        if current >= total_sectors {
            scan.diagnostics
                .push(VolumeDiagnostic::ExtendedPartitionInvalid {
                    lba: current,
                    reason: "outside the source".into(),
                });
            return;
        }
        let sector = match reader.read_sector(current) {
            Ok(s) => s,
            Err(e) => {
                scan.diagnostics
                    .push(VolumeDiagnostic::ExtendedPartitionInvalid {
                        lba: current,
                        reason: e.to_string(),
                    });
                return;
            }
        };
        let Some(ebr) = MbrSector::parse(&sector) else {
            scan.diagnostics
                .push(VolumeDiagnostic::ExtendedPartitionInvalid {
                    lba: current,
                    reason: "sector too small".into(),
                });
            return;
        };
        if !ebr.has_signature {
            scan.diagnostics
                .push(VolumeDiagnostic::ExtendedPartitionInvalid {
                    lba: current,
                    reason: "missing 55 AA signature".into(),
                });
            return;
        }

        // Entry 0: the logical partition, relative to this EBR.
        let logical = ebr.entries.first().copied().unwrap_or(MbrEntry {
            status: 0,
            partition_type: 0,
            chs_first: [0; 3],
            chs_last: [0; 3],
            first_lba: 0,
            sectors: 0,
        });
        if !logical.is_empty() {
            if logical.sectors == 0 {
                scan.diagnostics
                    .push(VolumeDiagnostic::ZeroLengthPartition { index: *next_index });
            } else {
                let start = current.saturating_add(u64::from(logical.first_lba));
                match arith::add(start, u64::from(logical.sectors)) {
                    Ok(end_excl) => {
                        if let Ok(mut p) = Partition::from_lba(
                            *next_index,
                            start,
                            end_excl - 1,
                            sector_size,
                            PartitionType::Mbr(logical.partition_type),
                        ) {
                            p.flags |= PartitionFlags::LOGICAL;
                            if logical.status == 0x80 {
                                p.flags |= PartitionFlags::BOOTABLE;
                            }
                            if start >= container_end || end_excl > container_end {
                                p.confidence = PartitionConfidence::Low;
                            }
                            scan.partitions.push(p);
                        }
                    }
                    Err(_) => scan
                        .diagnostics
                        .push(VolumeDiagnostic::ExtendedPartitionInvalid {
                            lba: current,
                            reason: "logical partition overflows".into(),
                        }),
                }
            }
            *next_index += 1;
        }

        // Entry 1: the next EBR, relative to the container start.
        let Some(next) = ebr.entries.get(1).copied() else {
            return;
        };
        if next.is_empty() || !next.is_extended() || next.sectors == 0 {
            return;
        }
        let next_lba = container_start.saturating_add(u64::from(next.first_lba));
        if next_lba == current {
            scan.diagnostics
                .push(VolumeDiagnostic::ExtendedPartitionLoop);
            return;
        }
        current = next_lba;
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builders for synthetic MBR sectors used across the crate's tests.

    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    /// Builds a 512-byte MBR/EBR sector from `(status, type, first_lba, sectors)` entries.
    #[must_use]
    pub fn sector(entries: &[(u8, u8, u32, u32)], signature: bool) -> Vec<u8> {
        let mut s = vec![0u8; 512];
        for (i, (status, ty, lba, count)) in entries.iter().enumerate().take(4) {
            let b = 446 + i * 16;
            s[b] = *status;
            s[b + 4] = *ty;
            s[b + 8..b + 12].copy_from_slice(&lba.to_le_bytes());
            s[b + 12..b + 16].copy_from_slice(&count.to_le_bytes());
        }
        if signature {
            s[510] = 0x55;
            s[511] = 0xAA;
        }
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

    use super::testutil::sector;
    use super::*;
    use phoinix_block::MemoryReader;

    fn disk(sectors: u64) -> Vec<u8> {
        vec![0u8; (sectors * 512) as usize]
    }

    #[test]
    fn parses_entries_and_signature() {
        let s = sector(&[(0x80, 0x07, 2048, 4096)], true);
        let m = MbrSector::parse(&s).unwrap();
        assert!(m.has_signature);
        assert_eq!(m.entries[0].status, 0x80);
        assert_eq!(m.entries[0].partition_type, 0x07);
        assert_eq!(m.entries[0].first_lba, 2048);
        assert_eq!(m.entries[0].sectors, 4096);
        assert!(m.entries[1].is_empty());
        assert!(MbrSector::parse(&s[..511]).is_none());
        assert!(!MbrSector::parse(&sector(&[], false)).unwrap().has_signature);
    }

    #[test]
    fn scans_primary_partitions() {
        let mut d = disk(16384);
        d[..512].copy_from_slice(&sector(
            &[(0x80, 0x07, 2048, 4096), (0, 0x83, 6144, 8192)],
            true,
        ));
        let r = MemoryReader::new(d);
        let mbr = MbrSector::parse(&r.data()[..512]).unwrap();
        let scan = scan_mbr(&r, &mbr).unwrap();
        assert_eq!(scan.partitions.len(), 2);
        assert_eq!(scan.partitions[0].start_lba, 2048);
        assert_eq!(scan.partitions[0].end_lba, 6143);
        assert_eq!(scan.partitions[0].start_offset, 2048 * 512);
        assert_eq!(scan.partitions[0].length, 4096 * 512);
        assert!(scan.partitions[0].flags.contains(PartitionFlags::BOOTABLE));
        assert_eq!(scan.partitions[1].index, 2);
        assert!(scan.diagnostics.is_empty());
    }

    #[test]
    fn follows_extended_chain() {
        // Extended container at 4096..16384 with two logical partitions.
        let mut d = disk(16384);
        d[..512].copy_from_slice(&sector(
            &[(0, 0x07, 2048, 2048), (0, 0x05, 4096, 12288)],
            true,
        ));
        // EBR 1 at 4096: logical at +2048 (2048 sectors), next EBR at container+6144.
        let e1 = 4096 * 512;
        d[e1..e1 + 512].copy_from_slice(&sector(
            &[(0, 0x83, 2048, 2048), (0, 0x05, 6144, 4096)],
            true,
        ));
        // EBR 2 at 10240: logical at +2048 (2048 sectors), end of chain.
        let e2 = 10240 * 512;
        d[e2..e2 + 512].copy_from_slice(&sector(&[(0, 0x83, 2048, 2048)], true));
        let r = MemoryReader::new(d);
        let mbr = MbrSector::parse(&r.data()[..512]).unwrap();
        let scan = scan_mbr(&r, &mbr).unwrap();
        let logical: Vec<_> = scan
            .partitions
            .iter()
            .filter(|p| p.flags.contains(PartitionFlags::LOGICAL))
            .collect();
        assert_eq!(logical.len(), 2, "{scan:?}");
        assert_eq!(logical[0].index, 5);
        assert_eq!(logical[0].start_lba, 4096 + 2048);
        assert_eq!(logical[1].index, 6);
        assert_eq!(logical[1].start_lba, 10240 + 2048);
        assert!(scan.diagnostics.is_empty(), "{:?}", scan.diagnostics);
    }

    #[test]
    fn extended_loop_is_detected() {
        let mut d = disk(16384);
        d[..512].copy_from_slice(&sector(&[(0, 0x05, 4096, 12288)], true));
        // EBR points back to itself (next EBR at container+0).
        let e1 = 4096 * 512;
        d[e1..e1 + 512]
            .copy_from_slice(&sector(&[(0, 0x83, 2048, 1024), (0, 0x05, 0, 4096)], true));
        let r = MemoryReader::new(d);
        let mbr = MbrSector::parse(&r.data()[..512]).unwrap();
        let scan = scan_mbr(&r, &mbr).unwrap();
        assert!(
            scan.diagnostics
                .contains(&VolumeDiagnostic::ExtendedPartitionLoop)
        );
        assert_eq!(scan.partitions.len(), 2);
    }

    #[test]
    fn extended_two_node_cycle_is_detected() {
        let mut d = disk(16384);
        d[..512].copy_from_slice(&sector(&[(0, 0x05, 4096, 12288)], true));
        let e1 = 4096 * 512;
        d[e1..e1 + 512].copy_from_slice(&sector(
            &[(0, 0x83, 2048, 1024), (0, 0x05, 4096, 4096)],
            true,
        ));
        let e2 = 8192 * 512;
        d[e2..e2 + 512]
            .copy_from_slice(&sector(&[(0, 0x83, 2048, 1024), (0, 0x05, 0, 4096)], true));
        let r = MemoryReader::new(d);
        let mbr = MbrSector::parse(&r.data()[..512]).unwrap();
        let scan = scan_mbr(&r, &mbr).unwrap();
        assert!(
            scan.diagnostics
                .contains(&VolumeDiagnostic::ExtendedPartitionLoop)
        );
    }

    #[test]
    fn extended_outside_source_is_reported() {
        let mut d = disk(4096);
        d[..512].copy_from_slice(&sector(&[(0, 0x05, 8192, 4096)], true));
        let r = MemoryReader::new(d);
        let mbr = MbrSector::parse(&r.data()[..512]).unwrap();
        let scan = scan_mbr(&r, &mbr).unwrap();
        assert!(matches!(
            scan.diagnostics[0],
            VolumeDiagnostic::ExtendedPartitionInvalid { lba: 8192, .. }
        ));
    }

    #[test]
    fn ebr_without_signature_stops_chain() {
        let mut d = disk(16384);
        d[..512].copy_from_slice(&sector(&[(0, 0x05, 4096, 12288)], true));
        let e1 = 4096 * 512;
        d[e1..e1 + 512].copy_from_slice(&sector(&[(0, 0x83, 2048, 1024)], false));
        let r = MemoryReader::new(d);
        let mbr = MbrSector::parse(&r.data()[..512]).unwrap();
        let scan = scan_mbr(&r, &mbr).unwrap();
        assert_eq!(scan.partitions.len(), 1);
        assert!(matches!(
            scan.diagnostics[0],
            VolumeDiagnostic::ExtendedPartitionInvalid { lba: 4096, .. }
        ));
    }

    #[test]
    fn plausibility_rejects_boot_code() {
        // Simulate boot code: garbage statuses.
        let mut s = sector(&[(0x33, 0x07, 2048, 100)], true);
        s[446] = 0x33;
        let m = MbrSector::parse(&s).unwrap();
        assert!(!m.entries_plausible(1_000_000));
        let ok = MbrSector::parse(&sector(&[(0x80, 0x07, 2048, 100)], true)).unwrap();
        assert!(ok.entries_plausible(1_000_000));
        assert!(
            !MbrSector::parse(&sector(&[], true))
                .unwrap()
                .entries_plausible(1_000_000)
        );
    }
}
