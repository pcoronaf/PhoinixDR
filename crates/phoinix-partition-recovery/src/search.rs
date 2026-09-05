//! The structure search: boot sectors and superblocks anywhere on the
//! source, interpreted into candidates with boundaries and evidence.

use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_carve::scanner::{ScanOptions, ScanProgress, find_headers};
use phoinix_carve::signature::{AssemblerKind, CarveSignature, HeaderPattern, SignatureSet};
use phoinix_core::ByteRange as BlockRange;
use phoinix_core::FileSystemType;
use phoinix_core::bytes::ByteView;
use phoinix_fs::{ByteRange, ProbeEvidence, ProbeRegistry};
use phoinix_fs_exfat::{ExFatProbe, ExfatBootSector, ExfatVolume};
use phoinix_fs_ext::{ExtProbe, ExtVolume};
use phoinix_fs_fat::{FatBootSector, FatProbe, FatVariant, FatVolume};
use phoinix_fs_ntfs::{NtfsBootSector, NtfsProbe, NtfsVolume};
use phoinix_volume::PartitionTable;

use crate::PartitionRecoveryError;
use crate::candidate::{FoundVia, PartitionCandidate, Relation, Repair, open_range};

/// Search parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    /// Offsets tested are multiples of this (512 = every sector).
    pub alignment: u64,
    /// Open each candidate with its filesystem engine to verify it.
    pub verify: bool,
    /// Stop after this many candidates.
    pub max_candidates: usize,
    /// Worker threads for matching (0 = all cores).
    pub threads: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            alignment: 512,
            verify: true,
            max_candidates: 10_000,
            threads: 0,
        }
    }
}

/// Signature ids used by the search.
const SIG_NTFS: &str = "ntfs-boot";
const SIG_EXFAT: &str = "exfat-boot";
const SIG_FAT_EB: &str = "fat-boot-eb";
const SIG_FAT_E9: &str = "fat-boot-e9";
const SIG_EXT: &str = "ext-superblock";

fn sig(id: &str, headers: &[(u32, &[u8])]) -> CarveSignature {
    CarveSignature {
        id: id.into(),
        name: id.into(),
        extension: String::new(),
        headers: headers
            .iter()
            .map(|(offset, bytes)| HeaderPattern {
                offset: *offset,
                bytes: bytes.to_vec(),
            })
            .collect(),
        footer: None,
        min_size: 0,
        max_size: 1,
        assembler: AssemblerKind::HeaderOnly,
    }
}

/// The structure signatures.
#[must_use]
pub fn structure_signatures() -> SignatureSet {
    SignatureSet::from_signatures(vec![
        sig(SIG_NTFS, &[(3, b"NTFS    ")]),
        sig(SIG_EXFAT, &[(3, b"EXFAT   ")]),
        sig(
            SIG_FAT_EB,
            &[(0, &[0xEB]), (2, &[0x90]), (510, &[0x55, 0xAA])],
        ),
        sig(SIG_FAT_E9, &[(0, &[0xE9]), (510, &[0x55, 0xAA])]),
        // Superblock at +1024, magic 0xEF53 little-endian at +56.
        sig(SIG_EXT, &[(1080, &[0x53, 0xEF])]),
    ])
}

/// A parsed structure before it becomes a candidate.
#[derive(Debug, Clone)]
struct Structure {
    start: u64,
    length: u64,
    filesystem: FileSystemType,
    label: Option<String>,
    serial: Option<String>,
    cluster_size: Option<u32>,
    sector_size: u32,
    found_via: FoundVia,
    primary_valid: bool,
    backup_valid: Option<bool>,
    evidence: Vec<ProbeEvidence>,
    repairs: Vec<Repair>,
}

/// Whether the `$MFT` sits where a boot sector at `start` says: the
/// discriminator between a primary boot sector and a stray backup.
fn ntfs_plausible(reader: &dyn BlockReader, start: u64, boot: &NtfsBootSector) -> bool {
    boot.mft_lcn
        .checked_mul(u64::from(boot.cluster_size))
        .and_then(|off| start.checked_add(off))
        .and_then(|at| reader.read_vec(at, 4).ok())
        .is_some_and(|b| b == b"FILE")
}

/// Whether the first FAT sits where a boot sector at `start` says (its
/// first byte is the media descriptor).
fn fat_plausible(reader: &dyn BlockReader, start: u64, boot: &FatBootSector) -> bool {
    start
        .checked_add(boot.fat_offset)
        .and_then(|at| reader.read_vec(at, 1).ok())
        .is_some_and(|b| b.first() == Some(&boot.media))
}

/// Whether the FAT sits where an exFAT boot sector at `start` says (its
/// first entry is the media descriptor entry `F8 FF FF FF`).
fn exfat_plausible(reader: &dyn BlockReader, start: u64, boot: &ExfatBootSector) -> bool {
    u64::from(boot.fat_offset)
        .checked_mul(u64::from(boot.bytes_per_sector))
        .and_then(|off| start.checked_add(off))
        .and_then(|at| reader.read_vec(at, 4).ok())
        .is_some_and(|b| b == [0xF8, 0xFF, 0xFF, 0xFF])
}

fn same_sector(reader: &dyn BlockReader, a: u64, b: u64, len: usize) -> Option<bool> {
    let x = reader.read_vec(a, len).ok()?;
    let y = reader.read_vec(b, len).ok()?;
    Some(x == y)
}

/// Interprets an NTFS boot sector at `hit`.
fn ntfs_structure(reader: &dyn BlockReader, hit: u64, sector: &[u8]) -> Option<Structure> {
    let boot = NtfsBootSector::parse(sector).ok()?;
    let bps = u64::from(boot.bytes_per_sector);
    let length = boot.total_sectors.checked_mul(bps)?;
    let serial = Some(format!("{:016X}", boot.volume_serial));
    let mut evidence = vec![ProbeEvidence::supports(format!(
        "NTFS boot sector: {} sectors of {} bytes, {}-byte clusters",
        boot.total_sectors, boot.bytes_per_sector, boot.cluster_size
    ))];
    let base = |start, found_via, primary_valid, backup_valid, evidence, repairs| Structure {
        start,
        length,
        filesystem: FileSystemType::Ntfs,
        label: None,
        serial: serial.clone(),
        cluster_size: Some(boot.cluster_size),
        sector_size: u32::from(boot.bytes_per_sector),
        found_via,
        primary_valid,
        backup_valid,
        evidence,
        repairs,
    };
    let primary_here = ntfs_plausible(reader, hit, &boot);
    if !primary_here {
        // Perhaps the backup boot sector of a volume whose primary is gone.
        if let Some(primary_at) = hit.checked_sub(length) {
            let primary = reader.read_vec(primary_at, sector.len()).ok();
            if primary.as_deref() == Some(sector) {
                // The primary exists and is identical: it produces the
                // candidate itself.
                return None;
            }
            if ntfs_plausible(reader, primary_at, &boot) {
                evidence.push(ProbeEvidence::supports(
                    "found through the backup boot sector; the primary boot sector is missing or damaged",
                ));
                evidence.push(ProbeEvidence::supports(
                    "the $MFT lies where the boot sector says",
                ));
                let repairs = vec![Repair {
                    offset: 0,
                    bytes: sector.to_vec(),
                    description: "backup boot sector substituted for the destroyed primary".into(),
                }];
                return Some(base(
                    primary_at,
                    FoundVia::BackupBootSector,
                    false,
                    Some(true),
                    evidence,
                    repairs,
                ));
            }
        }
        evidence.push(ProbeEvidence::contradicts(
            "the $MFT does not lie where the boot sector says; a stray or stale boot sector",
        ));
    } else {
        evidence.push(ProbeEvidence::supports(
            "the $MFT lies where the boot sector says",
        ));
    }
    let backup_valid = same_sector(reader, hit, hit.checked_add(length)?, sector.len());
    match backup_valid {
        Some(true) => evidence.push(ProbeEvidence::supports(
            "the backup boot sector at the end of the volume matches",
        )),
        Some(false) => evidence.push(ProbeEvidence::contradicts(
            "the backup boot sector at the end of the volume does not match",
        )),
        None => evidence.push(ProbeEvidence::contradicts(
            "the backup boot sector lies beyond the end of the source",
        )),
    }
    Some(base(
        hit,
        FoundVia::PrimaryBootSector,
        primary_here,
        backup_valid,
        evidence,
        Vec::new(),
    ))
}

/// Interprets an exFAT boot sector at `hit` (backup region at sector 12).
fn exfat_structure(reader: &dyn BlockReader, hit: u64, sector: &[u8]) -> Option<Structure> {
    let boot = ExfatBootSector::parse(sector).ok()?;
    let bps = u64::from(boot.bytes_per_sector);
    let length = boot.volume_length.checked_mul(bps)?;
    let backup_offset = bps.checked_mul(12)?;
    let serial = Some(format!("{:08X}", boot.volume_serial));
    let mut evidence = vec![ProbeEvidence::supports(format!(
        "exFAT boot sector: {} sectors of {} bytes, {}-byte clusters",
        boot.volume_length, boot.bytes_per_sector, boot.cluster_size
    ))];
    let base = |start, found_via, primary_valid, backup_valid, evidence, repairs| Structure {
        start,
        length,
        filesystem: FileSystemType::ExFat,
        label: None,
        serial: serial.clone(),
        cluster_size: Some(boot.cluster_size),
        sector_size: boot.bytes_per_sector,
        found_via,
        primary_valid,
        backup_valid,
        evidence,
        repairs,
    };
    let primary_here = exfat_plausible(reader, hit, &boot);
    if !primary_here {
        if let Some(primary_at) = hit.checked_sub(backup_offset) {
            let primary = reader.read_vec(primary_at, sector.len()).ok();
            if primary.as_deref() == Some(sector) {
                return None;
            }
            if exfat_plausible(reader, primary_at, &boot) {
                evidence.push(ProbeEvidence::supports(
                    "found through the backup boot region; the primary boot sector is missing or damaged",
                ));
                let repairs = vec![Repair {
                    offset: 0,
                    bytes: sector.to_vec(),
                    description: "backup boot sector substituted for the destroyed primary".into(),
                }];
                return Some(base(
                    primary_at,
                    FoundVia::BackupBootSector,
                    false,
                    Some(true),
                    evidence,
                    repairs,
                ));
            }
        }
        evidence.push(ProbeEvidence::contradicts(
            "the FAT does not lie where the boot sector says; a stray or stale boot sector",
        ));
    } else {
        evidence.push(ProbeEvidence::supports(
            "the FAT lies where the boot sector says",
        ));
    }
    let backup_valid = same_sector(reader, hit, hit.checked_add(backup_offset)?, sector.len());
    match backup_valid {
        Some(true) => evidence.push(ProbeEvidence::supports(
            "the backup boot region at sector 12 matches",
        )),
        Some(false) => evidence.push(ProbeEvidence::contradicts(
            "the backup boot region at sector 12 does not match",
        )),
        None => {}
    }
    Some(base(
        hit,
        FoundVia::PrimaryBootSector,
        primary_here,
        backup_valid,
        evidence,
        Vec::new(),
    ))
}

/// Interprets a FAT boot sector at `hit` (FAT32 backup at sector 6).
fn fat_structure(reader: &dyn BlockReader, hit: u64, sector: &[u8]) -> Option<Structure> {
    let boot = FatBootSector::parse(sector).ok()?;
    let bps = u64::from(boot.bytes_per_sector);
    let length = boot.total_sectors.checked_mul(bps)?;
    let filesystem = boot.variant.filesystem_type();
    let label =
        Some(boot.volume_label.trim().to_owned()).filter(|l| !l.is_empty() && l != "NO NAME");
    let serial = Some(format!(
        "{:04X}-{:04X}",
        boot.volume_serial >> 16,
        boot.volume_serial & 0xFFFF
    ));
    let mut evidence = vec![ProbeEvidence::supports(format!(
        "{} boot sector: {} sectors of {} bytes, {}-byte clusters, {} data clusters",
        filesystem,
        boot.total_sectors,
        boot.bytes_per_sector,
        boot.cluster_size,
        boot.cluster_count
    ))];
    let base = |start, found_via, primary_valid, backup_valid, evidence, repairs| Structure {
        start,
        length,
        filesystem,
        label: label.clone(),
        serial: serial.clone(),
        cluster_size: Some(boot.cluster_size),
        sector_size: u32::from(boot.bytes_per_sector),
        found_via,
        primary_valid,
        backup_valid,
        evidence,
        repairs,
    };
    let backup_offset = bps.checked_mul(6)?;
    let primary_here = fat_plausible(reader, hit, &boot);
    if !primary_here {
        if boot.variant == FatVariant::Fat32
            && let Some(primary_at) = hit.checked_sub(backup_offset)
        {
            let primary = reader.read_vec(primary_at, sector.len()).ok();
            if primary.as_deref() == Some(sector) {
                return None;
            }
            if fat_plausible(reader, primary_at, &boot) {
                evidence.push(ProbeEvidence::supports(
                    "found through the backup boot sector at sector 6; the primary boot sector is missing or damaged",
                ));
                let repairs = vec![Repair {
                    offset: 0,
                    bytes: sector.to_vec(),
                    description: "backup boot sector substituted for the destroyed primary".into(),
                }];
                return Some(base(
                    primary_at,
                    FoundVia::BackupBootSector,
                    false,
                    Some(true),
                    evidence,
                    repairs,
                ));
            }
        }
        evidence.push(ProbeEvidence::contradicts(
            "the FAT does not lie where the boot sector says; a stray or stale boot sector",
        ));
    } else {
        evidence.push(ProbeEvidence::supports(
            "the FAT lies where the boot sector says",
        ));
    }
    let backup_valid = if boot.variant == FatVariant::Fat32 {
        let b = same_sector(reader, hit, hit.checked_add(backup_offset)?, sector.len());
        match b {
            Some(true) => evidence.push(ProbeEvidence::supports(
                "the backup boot sector at sector 6 matches",
            )),
            Some(false) => evidence.push(ProbeEvidence::contradicts(
                "the backup boot sector at sector 6 does not match",
            )),
            None => {}
        }
        b
    } else {
        None
    };
    Some(base(
        hit,
        FoundVia::PrimaryBootSector,
        primary_here,
        backup_valid,
        evidence,
        Vec::new(),
    ))
}

/// A minimal EXT superblock reading.
#[derive(Debug, Clone)]
struct ExtSuperblock {
    block_size: u64,
    blocks: u64,
    blocks_per_group: u64,
    group: u16,
    label: Option<String>,
    uuid: Option<String>,
    flavour: &'static str,
}

fn parse_ext_superblock(sb: &[u8]) -> Option<ExtSuperblock> {
    let v = ByteView::new(sb);
    if v.u16_le(56)? != 0xEF53 {
        return None;
    }
    let log_block = v.u32_le(24)?;
    if log_block > 6 {
        return None;
    }
    let block_size = 1024u64 << log_block;
    let inodes = v.u32_le(0)?;
    let blocks_per_group = u64::from(v.u32_le(32)?);
    let features_incompat = v.u32_le(96)?;
    let features_compat = v.u32_le(92)?;
    let mut blocks = u64::from(v.u32_le(4)?);
    if features_incompat & 0x0080 != 0 {
        blocks |= u64::from(v.u32_le(0x150)?) << 32;
    }
    if inodes == 0 || blocks == 0 || blocks_per_group == 0 {
        return None;
    }
    let group = v.u16_le(0x5A)?;
    let label = v
        .slice(0x78, 16)
        .map(|b| {
            String::from_utf8_lossy(b)
                .trim_end_matches('\0')
                .trim()
                .to_owned()
        })
        .filter(|s| !s.is_empty());
    let uuid = v
        .slice(0x68, 16)
        .and_then(|b| uuid::Uuid::from_slice(b).ok())
        .filter(|u| !u.is_nil())
        .map(|u| u.to_string());
    let flavour = if features_incompat & 0x0040 != 0 {
        "ext4"
    } else if features_compat & 0x0004 != 0 {
        "ext3"
    } else {
        "ext2"
    };
    Some(ExtSuperblock {
        block_size,
        blocks,
        blocks_per_group,
        group,
        label,
        uuid,
        flavour,
    })
}

/// Interprets an EXT superblock whose magic matched at `hit + 1080`.
fn ext_structure(reader: &dyn BlockReader, hit: u64) -> Option<Structure> {
    let sb_at = hit.checked_add(1024)?;
    let bytes = reader.read_vec(sb_at, 1024).ok()?;
    let sb = parse_ext_superblock(&bytes)?;
    let length = sb.blocks.checked_mul(sb.block_size)?;
    let mut evidence = vec![ProbeEvidence::supports(format!(
        "{} superblock: {} blocks of {} bytes, {} blocks per group",
        sb.flavour, sb.blocks, sb.block_size, sb.blocks_per_group
    ))];
    if sb.group == 0 {
        // Primary; is there a backup in group 1?
        let backup_at = if sb.block_size == 1024 {
            hit.checked_add(sb.blocks_per_group.checked_mul(1024)?)?
                .checked_add(1024)?
        } else {
            hit.checked_add(sb.blocks_per_group.checked_mul(sb.block_size)?)?
        };
        let backup_valid = reader
            .read_vec(backup_at, 1024)
            .ok()
            .and_then(|b| parse_ext_superblock(&b))
            .map(|b| b.blocks == sb.blocks && b.uuid == sb.uuid);
        match backup_valid {
            Some(true) => evidence.push(ProbeEvidence::supports(
                "the backup superblock of block group 1 matches",
            )),
            Some(false) => evidence.push(ProbeEvidence::contradicts(
                "the backup superblock of block group 1 does not match",
            )),
            None => {}
        }
        return Some(Structure {
            start: hit,
            length,
            filesystem: FileSystemType::Ext,
            label: sb.label,
            serial: sb.uuid,
            cluster_size: u32::try_from(sb.block_size).ok(),
            sector_size: 512,
            found_via: FoundVia::Superblock,
            primary_valid: true,
            backup_valid,
            evidence,
            repairs: Vec::new(),
        });
    }
    // A backup copy: derive the volume start from its group number.
    let group_bytes = u64::from(sb.group)
        .checked_mul(sb.blocks_per_group)?
        .checked_mul(sb.block_size)?;
    let start = if sb.block_size == 1024 {
        sb_at.checked_sub(group_bytes)?.checked_sub(1024)?
    } else {
        sb_at.checked_sub(group_bytes)?
    };
    // If the primary is intact, it produces the candidate.
    if let Ok(primary) = reader.read_vec(start.checked_add(1024)?, 1024)
        && let Some(p) = parse_ext_superblock(&primary)
        && p.group == 0
        && p.uuid == sb.uuid
    {
        return None;
    }
    evidence.push(ProbeEvidence::supports(format!(
        "found through the backup superblock of block group {}; the primary superblock is missing or damaged",
        sb.group
    )));
    Some(Structure {
        start,
        length,
        filesystem: FileSystemType::Ext,
        label: sb.label,
        serial: sb.uuid,
        cluster_size: u32::try_from(sb.block_size).ok(),
        sector_size: 512,
        found_via: FoundVia::BackupSuperblock,
        primary_valid: false,
        backup_valid: Some(true),
        evidence,
        repairs: vec![Repair {
            offset: 1024,
            bytes,
            description: format!(
                "backup superblock of block group {} substituted for the destroyed primary",
                sb.group
            ),
        }],
    })
}

/// Searches `reader` for filesystem structures and returns candidates in
/// start order, related to `table` when given.
///
/// # Errors
///
/// Returns [`PartitionRecoveryError`] for I/O failures.
pub fn find_partitions(
    reader: &Arc<dyn BlockReader>,
    table: Option<&PartitionTable>,
    options: &SearchOptions,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<Vec<PartitionCandidate>, PartitionRecoveryError> {
    let signatures = structure_signatures();
    let scan = ScanOptions {
        alignment: options.alignment.max(1),
        threads: options.threads,
        max_hits: 1_000_000,
        ..Default::default()
    };
    let ranges = [ByteRange {
        offset: 0,
        length: reader.len(),
    }];
    let hits = find_headers(&**reader, &ranges, &signatures, &scan, progress)?;
    tracing::info!(hits = hits.len(), "structure hits");
    let source_len = reader.len();
    let source_sector = u64::from(reader.geometry().logical_sector_size.max(1));
    let mut structures: Vec<Structure> = Vec::new();
    for hit in hits {
        let Some(signature) = signatures.get(hit.signature) else {
            continue;
        };
        let sector = match reader.read_vec(hit.offset, 512) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let found = match signature.id.as_str() {
            SIG_NTFS => ntfs_structure(&**reader, hit.offset, &sector),
            SIG_EXFAT => exfat_structure(&**reader, hit.offset, &sector),
            SIG_FAT_EB | SIG_FAT_E9 => fat_structure(&**reader, hit.offset, &sector),
            SIG_EXT => ext_structure(&**reader, hit.offset),
            _ => None,
        };
        let Some(s) = found else {
            continue;
        };
        if s.length == 0 {
            continue;
        }
        if let Some(existing) = structures
            .iter_mut()
            .find(|e| e.start == s.start && e.filesystem == s.filesystem)
        {
            // The same volume seen twice (primary and backup): keep the
            // primary's view, add the evidence.
            for ev in s.evidence {
                if !existing.evidence.contains(&ev) {
                    existing.evidence.push(ev);
                }
            }
            if s.primary_valid && !existing.primary_valid {
                existing.repairs.clear();
                existing.found_via = s.found_via;
            }
            existing.primary_valid |= s.primary_valid;
            if s.backup_valid == Some(true) {
                existing.backup_valid = Some(true);
            }
            continue;
        }
        structures.push(s);
        if structures.len() >= options.max_candidates {
            tracing::warn!(max = options.max_candidates, "candidate limit reached");
            break;
        }
    }
    structures.sort_by_key(|s| (s.start, s.length));

    // ---- candidates with geometry, verification, relations, confidence ----
    let mut candidates: Vec<PartitionCandidate> = Vec::with_capacity(structures.len());
    for s in structures {
        let readable = s.length.min(source_len.saturating_sub(s.start));
        let fits = s
            .start
            .checked_add(s.length)
            .is_some_and(|end| end <= source_len);
        let aligned = s.start % source_sector == 0;
        let sector_ok = u64::from(s.sector_size) == source_sector || s.sector_size == 512;
        let mut evidence = s.evidence;
        if !fits {
            evidence.push(ProbeEvidence::contradicts(format!(
                "the declared length runs {} bytes past the end of the source",
                s.start.saturating_add(s.length).saturating_sub(source_len)
            )));
        }
        if !aligned {
            evidence.push(ProbeEvidence::contradicts(
                "the start is not sector-aligned on this source",
            ));
        }
        let mut engine_verified = None;
        let mut root_entries = None;
        let mut probe_confidence = 0u8;
        if options.verify && readable > 0 {
            let range = BlockRange {
                offset: s.start,
                length: readable,
            };
            if let Ok(view) = open_range(reader.clone(), range, &s.repairs) {
                let detection = ProbeRegistry::new()
                    .with(Box::new(NtfsProbe))
                    .with(Box::new(FatProbe))
                    .with(Box::new(ExFatProbe))
                    .with(Box::new(ExtProbe))
                    .detect(&*view);
                if detection.filesystem() == s.filesystem {
                    probe_confidence = detection.best.as_ref().map_or(0, |b| b.confidence);
                } else {
                    evidence.push(ProbeEvidence::contradicts(format!(
                        "the filesystem probe on the volume start says {} rather than {}",
                        detection.filesystem(),
                        s.filesystem
                    )));
                }
                let (verified, entries) = verify_with_engine(&view, s.filesystem);
                engine_verified = verified;
                root_entries = entries;
                match verified {
                    Some(true) => evidence.push(ProbeEvidence::supports(format!(
                        "the {} engine opened the volume{}",
                        s.filesystem,
                        entries.map_or(String::new(), |n| format!(" and read {n} root entries"))
                    ))),
                    Some(false) => evidence.push(ProbeEvidence::contradicts(format!(
                        "the {} engine could not open the volume",
                        s.filesystem
                    ))),
                    None => {}
                }
            }
        }
        let geometry_consistent = fits && aligned && sector_ok;
        let mut confidence: i32 = if s.primary_valid { 60 } else { 45 };
        if s.backup_valid == Some(true) {
            confidence += 15;
        }
        if s.backup_valid == Some(false) {
            confidence -= 10;
        }
        confidence += i32::from(probe_confidence) / 10;
        match engine_verified {
            Some(true) => confidence += 15,
            Some(false) => confidence -= 30,
            None => {}
        }
        if !geometry_consistent {
            confidence -= 15;
        }
        if !fits {
            confidence = confidence.min(60);
        }
        candidates.push(PartitionCandidate {
            start: s.start,
            length: s.length,
            readable_length: readable,
            filesystem: s.filesystem,
            label: s.label,
            serial: s.serial,
            cluster_size: s.cluster_size,
            sector_size: s.sector_size,
            found_via: s.found_via,
            primary_structure_valid: s.primary_valid,
            backup_structure_valid: s.backup_valid,
            geometry_consistent,
            engine_verified,
            root_entries,
            relation: Relation::Lost,
            repairs: s.repairs,
            evidence,
            confidence: u8::try_from(confidence.clamp(0, 100)).unwrap_or(0),
        });
    }

    // ---- relations -------------------------------------------------------
    let snapshot = candidates.clone();
    for (i, c) in candidates.iter_mut().enumerate() {
        let mut relation = Relation::Lost;
        if let Some(t) = table {
            for p in &t.partitions {
                let exact = p.start_offset == c.start
                    && (p.length == c.length
                        || p.length.abs_diff(c.length) <= u64::from(c.sector_size));
                if exact {
                    relation = Relation::Listed { index: p.index };
                    break;
                }
                if c.start >= p.start_offset && c.end() <= p.start_offset.saturating_add(p.length) {
                    relation = Relation::InsidePartition { index: p.index };
                }
            }
        }
        if matches!(relation, Relation::Lost | Relation::InsidePartition { .. }) {
            for (j, other) in snapshot.iter().enumerate() {
                if i == j {
                    continue;
                }
                if c.inside(other) {
                    relation = Relation::Nested { within: j };
                    c.evidence.push(ProbeEvidence::contradicts(format!(
                        "lies inside candidate {} ({}); probably an image file stored on that volume",
                        j + 1,
                        other.filesystem
                    )));
                    c.confidence = c.confidence.saturating_sub(30);
                    break;
                }
                if c.overlaps(other) && !other.inside(c) && matches!(relation, Relation::Lost) {
                    relation = Relation::Overlapping { with: j };
                    c.evidence.push(ProbeEvidence::contradicts(format!(
                        "overlaps candidate {} ({}); one of them is a stale structure",
                        j + 1,
                        other.filesystem
                    )));
                    c.confidence = c.confidence.saturating_sub(15);
                }
            }
        }
        c.relation = relation;
    }
    Ok(candidates)
}

/// Opens the volume with its engine and reads the root directory.
fn verify_with_engine(
    view: &Arc<dyn BlockReader>,
    fs: FileSystemType,
) -> (Option<bool>, Option<usize>) {
    match fs {
        FileSystemType::Ntfs => match NtfsVolume::open(view.clone()) {
            Ok(v) => (Some(v.file(5).is_ok()), None),
            Err(_) => (Some(false), None),
        },
        FileSystemType::Fat12 | FileSystemType::Fat16 | FileSystemType::Fat32 => {
            match FatVolume::open(view.clone()) {
                Ok(v) => match v.root_directory() {
                    Ok(entries) => (
                        Some(true),
                        Some(entries.iter().filter(|e| !e.is_dot()).count()),
                    ),
                    Err(_) => (Some(false), None),
                },
                Err(_) => (Some(false), None),
            }
        }
        FileSystemType::ExFat => match ExfatVolume::open(view.clone()) {
            Ok(v) => match v.walk() {
                Ok(entries) => (Some(true), Some(entries.len())),
                Err(_) => (Some(false), None),
            },
            Err(_) => (Some(false), None),
        },
        FileSystemType::Ext => match ExtVolume::open(view.clone()) {
            Ok(v) => match v.inode(2).and_then(|root| v.layout_of(&root)) {
                Ok(layout) => match v.read_directory(&layout) {
                    Ok(entries) => (
                        Some(true),
                        Some(entries.iter().filter(|e| !e.deleted && !e.is_dot()).count()),
                    ),
                    Err(_) => (Some(false), None),
                },
                Err(_) => (Some(false), None),
            },
            Err(_) => (Some(false), None),
        },
        _ => (None, None),
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
    fn ext_superblock_parsing() {
        let mut sb = vec![0u8; 1024];
        sb[56..58].copy_from_slice(&0xEF53u16.to_le_bytes());
        sb[0..4].copy_from_slice(&4096u32.to_le_bytes());
        sb[4..8].copy_from_slice(&16384u32.to_le_bytes());
        sb[24..28].copy_from_slice(&0u32.to_le_bytes());
        sb[32..36].copy_from_slice(&8192u32.to_le_bytes());
        sb[0x5A..0x5C].copy_from_slice(&3u16.to_le_bytes());
        sb[0x78..0x7F].copy_from_slice(b"PHXEXT4");
        sb[0x60..0x64].copy_from_slice(&0x0040u32.to_le_bytes());
        let p = parse_ext_superblock(&sb).unwrap();
        assert_eq!(
            (p.block_size, p.blocks, p.blocks_per_group, p.group),
            (1024, 16384, 8192, 3)
        );
        assert_eq!(p.label.as_deref(), Some("PHXEXT4"));
        assert_eq!(p.flavour, "ext4");
        assert!(p.uuid.is_none());
        sb[24] = 9;
        assert!(parse_ext_superblock(&sb).is_none());
    }

    #[test]
    fn signatures_match_boot_sectors() {
        let set = structure_signatures();
        let mut ntfs = vec![0u8; 512];
        ntfs[3..11].copy_from_slice(b"NTFS    ");
        assert_eq!(set.matches_at(&ntfs).count(), 1);
        let mut fat = vec![0u8; 512];
        fat[0] = 0xEB;
        fat[2] = 0x90;
        fat[510] = 0x55;
        fat[511] = 0xAA;
        assert_eq!(set.matches_at(&fat).count(), 1);
        let mut ext = vec![0u8; 1100];
        ext[1080] = 0x53;
        ext[1081] = 0xEF;
        assert_eq!(set.matches_at(&ext).count(), 1);
    }
}
