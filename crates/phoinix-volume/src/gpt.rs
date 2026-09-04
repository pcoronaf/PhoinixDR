//! GUID Partition Table parsing and validation.

use phoinix_block::{BlockReader, BlockReaderExt, MAX_SINGLE_READ};
use phoinix_core::arith;
use phoinix_core::bytes::{ByteView, utf16le_to_string, utf16le_to_string_lossy};
use uuid::Uuid;

use crate::{Partition, PartitionFlags, PartitionType, VolumeDiagnostic, VolumeError};

/// `EFI PART`.
pub const SIGNATURE: [u8; 8] = *b"EFI PART";
/// Smallest header size the specification allows.
pub const MIN_HEADER_SIZE: u32 = 92;
/// Smallest partition entry size the specification allows.
pub const MIN_ENTRY_SIZE: u32 = 128;
/// Upper bound on the partition entry array (protects allocations).
pub const MAX_ARRAY_BYTES: u64 = MAX_SINGLE_READ as u64;

/// A parsed and validated GPT header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptHeader {
    /// Revision (usually `0x0001_0000`).
    pub revision: u32,
    /// Header size in bytes.
    pub header_size: u32,
    /// Stored header CRC32.
    pub header_crc: u32,
    /// LBA this header claims to live at.
    pub current_lba: u64,
    /// LBA of the other header.
    pub backup_lba: u64,
    /// First usable LBA for partitions.
    pub first_usable_lba: u64,
    /// Last usable LBA for partitions (inclusive).
    pub last_usable_lba: u64,
    /// Disk GUID.
    pub disk_guid: Uuid,
    /// LBA of the partition entry array.
    pub entries_lba: u64,
    /// Number of entries in the array.
    pub entry_count: u32,
    /// Size of each entry.
    pub entry_size: u32,
    /// Stored CRC32 of the entry array.
    pub entries_crc: u32,
}

/// Why a GPT header was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GptHeaderError {
    /// Signature is not `EFI PART`.
    #[error("missing EFI PART signature")]
    Signature,
    /// The header is too short or longer than a sector.
    #[error("header size {0} is invalid")]
    HeaderSize(u32),
    /// The stored CRC32 does not match the computed one.
    #[error("header CRC32 mismatch (stored {stored:#010x}, computed {computed:#010x})")]
    Crc {
        /// Stored value.
        stored: u32,
        /// Computed value.
        computed: u32,
    },
    /// The entry count or size is unreasonable.
    #[error("partition entry geometry invalid: {count} entries of {size} bytes")]
    EntryGeometry {
        /// Entry count.
        count: u32,
        /// Entry size.
        size: u32,
    },
    /// The usable range is inconsistent with the source.
    #[error("usable LBA range {first}..={last} is invalid for {total_sectors} sectors")]
    UsableRange {
        /// First usable LBA.
        first: u64,
        /// Last usable LBA.
        last: u64,
        /// Total sectors on the source.
        total_sectors: u64,
    },
    /// The header's `current_lba` does not match where it was read from.
    #[error("header claims LBA {claimed} but was read from LBA {actual}")]
    Location {
        /// Claimed LBA.
        claimed: u64,
        /// Actual LBA.
        actual: u64,
    },
    /// The entry array does not fit inside the source.
    #[error("partition entry array at LBA {lba} lies outside the source")]
    ArrayOutsideSource {
        /// Array LBA.
        lba: u64,
    },
    /// The sector was shorter than the header.
    #[error("sector too short")]
    Truncated,
}

impl GptHeader {
    /// Parses and validates a header read from `actual_lba` on a source of
    /// `total_sectors` sectors of `sector_size` bytes.
    ///
    /// The CRC is computed with the CRC field treated as zero, as required
    /// by the specification.
    ///
    /// # Errors
    ///
    /// Returns a [`GptHeaderError`] describing the first failed check.
    pub fn parse(
        sector: &[u8],
        actual_lba: u64,
        sector_size: u32,
        total_sectors: u64,
    ) -> Result<Self, GptHeaderError> {
        let view = ByteView::new(sector);
        if view.array::<8>(0).ok_or(GptHeaderError::Truncated)? != SIGNATURE {
            return Err(GptHeaderError::Signature);
        }
        let revision = view.u32_le(8).ok_or(GptHeaderError::Truncated)?;
        let header_size = view.u32_le(12).ok_or(GptHeaderError::Truncated)?;
        if header_size < MIN_HEADER_SIZE || header_size > sector_size {
            return Err(GptHeaderError::HeaderSize(header_size));
        }
        let header_len =
            usize::try_from(header_size).map_err(|_| GptHeaderError::HeaderSize(header_size))?;
        let header_bytes = view.slice(0, header_len).ok_or(GptHeaderError::Truncated)?;
        let header_crc = view.u32_le(16).ok_or(GptHeaderError::Truncated)?;

        let mut scratch = header_bytes.to_vec();
        if let Some(field) = scratch.get_mut(16..20) {
            field.copy_from_slice(&[0, 0, 0, 0]);
        }
        let computed = crc32fast::hash(&scratch);
        if computed != header_crc {
            return Err(GptHeaderError::Crc {
                stored: header_crc,
                computed,
            });
        }

        let current_lba = view.u64_le(24).ok_or(GptHeaderError::Truncated)?;
        let backup_lba = view.u64_le(32).ok_or(GptHeaderError::Truncated)?;
        let first_usable_lba = view.u64_le(40).ok_or(GptHeaderError::Truncated)?;
        let last_usable_lba = view.u64_le(48).ok_or(GptHeaderError::Truncated)?;
        let disk_guid = Uuid::from_bytes_le(view.array::<16>(56).ok_or(GptHeaderError::Truncated)?);
        let entries_lba = view.u64_le(72).ok_or(GptHeaderError::Truncated)?;
        let entry_count = view.u32_le(80).ok_or(GptHeaderError::Truncated)?;
        let entry_size = view.u32_le(84).ok_or(GptHeaderError::Truncated)?;
        let entries_crc = view.u32_le(88).ok_or(GptHeaderError::Truncated)?;

        if current_lba != actual_lba {
            return Err(GptHeaderError::Location {
                claimed: current_lba,
                actual: actual_lba,
            });
        }
        let entry_geometry_ok = entry_size >= MIN_ENTRY_SIZE
            && entry_size.is_power_of_two()
            && entry_count > 0
            && u64::from(entry_count) * u64::from(entry_size) <= MAX_ARRAY_BYTES;
        if !entry_geometry_ok {
            return Err(GptHeaderError::EntryGeometry {
                count: entry_count,
                size: entry_size,
            });
        }
        if first_usable_lba > last_usable_lba || first_usable_lba == 0 {
            return Err(GptHeaderError::UsableRange {
                first: first_usable_lba,
                last: last_usable_lba,
                total_sectors,
            });
        }
        // Tolerate a truncated image: the usable range may exceed the source,
        // but the array we need to read must exist.
        let array_bytes = u64::from(entry_count) * u64::from(entry_size);
        let array_sectors =
            arith::div_ceil(array_bytes, u64::from(sector_size)).unwrap_or(u64::MAX);
        let array_end = entries_lba
            .checked_add(array_sectors)
            .ok_or(GptHeaderError::ArrayOutsideSource { lba: entries_lba })?;
        if entries_lba == 0 || array_end > total_sectors {
            return Err(GptHeaderError::ArrayOutsideSource { lba: entries_lba });
        }

        Ok(Self {
            revision,
            header_size,
            header_crc,
            current_lba,
            backup_lba,
            first_usable_lba,
            last_usable_lba,
            disk_guid,
            entries_lba,
            entry_count,
            entry_size,
            entries_crc,
        })
    }

    /// Byte length of the partition entry array.
    #[must_use]
    pub fn array_bytes(&self) -> u64 {
        u64::from(self.entry_count) * u64::from(self.entry_size)
    }
}

/// A raw GPT partition entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GptEntry {
    /// Partition type GUID (zero when unused).
    pub type_guid: Uuid,
    /// Unique partition GUID.
    pub unique_guid: Uuid,
    /// First LBA.
    pub first_lba: u64,
    /// Last LBA (inclusive).
    pub last_lba: u64,
    /// Attribute bits.
    pub attributes: u64,
    /// Name, decoded from UTF-16LE.
    pub name: String,
    /// Whether the name failed strict UTF-16 decoding.
    pub name_invalid_utf16: bool,
}

impl GptEntry {
    /// Parses one entry of at least 128 bytes.
    #[must_use]
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let view = ByteView::new(bytes);
        let type_guid = Uuid::from_bytes_le(view.array::<16>(0)?);
        let unique_guid = Uuid::from_bytes_le(view.array::<16>(16)?);
        let first_lba = view.u64_le(32)?;
        let last_lba = view.u64_le(40)?;
        let attributes = view.u64_le(48)?;
        let raw_name = view.slice(56, 72)?;
        // Trim at the first UTF-16 NUL.
        let mut end = raw_name.len();
        for (i, pair) in raw_name.chunks_exact(2).enumerate() {
            if pair == [0, 0] {
                end = i * 2;
                break;
            }
        }
        let name_bytes = raw_name.get(..end)?;
        let (name, name_invalid_utf16) = match utf16le_to_string(name_bytes) {
            Some(n) => (n, false),
            None => (utf16le_to_string_lossy(name_bytes), true),
        };
        Some(Self {
            type_guid,
            unique_guid,
            first_lba,
            last_lba,
            attributes,
            name,
            name_invalid_utf16,
        })
    }

    /// Whether this entry is unused.
    #[must_use]
    pub fn is_unused(&self) -> bool {
        self.type_guid.is_nil()
    }
}

/// Outcome of reading one GPT header and its entries.
#[derive(Debug)]
pub struct GptRead {
    /// The validated header.
    pub header: GptHeader,
    /// Raw entries (used ones only).
    pub entries: Vec<(u32, GptEntry)>,
    /// Whether the entry-array CRC matched.
    pub array_crc_ok: bool,
}

/// Reads the header at `lba` and, if valid, its partition array.
///
/// # Errors
///
/// Returns `Ok(Err(reason))` when the header is invalid, and `Err` only for
/// I/O failures.
pub fn read_gpt_at(
    reader: &dyn BlockReader,
    lba: u64,
) -> Result<Result<GptRead, GptHeaderError>, VolumeError> {
    let sector_size = reader.geometry().logical_sector_size;
    let total_sectors = reader.len() / u64::from(sector_size);
    if lba >= total_sectors {
        return Ok(Err(GptHeaderError::Truncated));
    }
    let sector = reader.read_sector(lba)?;
    let header = match GptHeader::parse(&sector, lba, sector_size, total_sectors) {
        Ok(h) => h,
        Err(e) => return Ok(Err(e)),
    };
    let array_offset = arith::mul(header.entries_lba, u64::from(sector_size))?;
    let array_len = arith::to_usize(header.array_bytes())?;
    let array = reader.read_vec(array_offset, array_len)?;
    let array_crc_ok = crc32fast::hash(&array) == header.entries_crc;
    let entry_size = usize::try_from(header.entry_size).map_err(|_| VolumeError::Overflow)?;
    let mut entries = Vec::new();
    for (i, chunk) in array.chunks_exact(entry_size).enumerate() {
        if let Some(entry) = GptEntry::parse(chunk)
            && !entry.is_unused()
        {
            entries.push((
                u32::try_from(i).unwrap_or(u32::MAX).saturating_add(1),
                entry,
            ));
        }
    }
    Ok(Ok(GptRead {
        header,
        entries,
        array_crc_ok,
    }))
}

/// Result of GPT discovery combining primary and backup headers.
#[derive(Debug)]
pub struct GptScan {
    /// The header that was used.
    pub header: GptHeader,
    /// Partitions.
    pub partitions: Vec<Partition>,
    /// Findings.
    pub diagnostics: Vec<VolumeDiagnostic>,
    /// Whether the backup header was used because the primary was invalid.
    pub used_backup: bool,
}

/// Attempts to read the GPT from LBA 1 and from the last LBA.
///
/// Returns [`None`] when neither header validates (diagnostics explaining why
/// are appended to `diagnostics`).
///
/// # Errors
///
/// Returns [`VolumeError`] for I/O failures.
pub fn scan_gpt(
    reader: &dyn BlockReader,
    diagnostics: &mut Vec<VolumeDiagnostic>,
) -> Result<Option<GptScan>, VolumeError> {
    let sector_size = reader.geometry().logical_sector_size;
    let total_sectors = reader.len() / u64::from(sector_size);
    if total_sectors < 2 {
        return Ok(None);
    }

    let primary = read_gpt_at(reader, 1)?;
    let backup_lba = match &primary {
        Ok(p) => p.header.backup_lba,
        Err(_) => total_sectors - 1,
    };
    let backup = if backup_lba < total_sectors && backup_lba != 1 {
        read_gpt_at(reader, backup_lba)?
    } else {
        Err(GptHeaderError::Truncated)
    };

    let mut used_backup = false;
    let chosen = match (primary, backup) {
        (Ok(p), Ok(b)) => {
            if p.header.disk_guid != b.header.disk_guid
                || p.header.first_usable_lba != b.header.first_usable_lba
                || p.header.last_usable_lba != b.header.last_usable_lba
            {
                diagnostics.push(VolumeDiagnostic::GptHeadersDisagree);
            }
            if !p.array_crc_ok && b.array_crc_ok {
                diagnostics.push(VolumeDiagnostic::GptArrayCrcMismatch);
                diagnostics.push(VolumeDiagnostic::BackupGptValid);
                used_backup = true;
                b
            } else {
                p
            }
        }
        (Ok(p), Err(reason)) => {
            diagnostics.push(VolumeDiagnostic::BackupGptInvalid {
                reason: reason.to_string(),
            });
            p
        }
        (Err(reason), Ok(b)) => {
            if matches!(reason, GptHeaderError::Crc { .. }) {
                diagnostics.push(VolumeDiagnostic::GptHeaderCrcMismatch);
            }
            diagnostics.push(VolumeDiagnostic::PrimaryGptInvalid {
                reason: reason.to_string(),
            });
            diagnostics.push(VolumeDiagnostic::BackupGptValid);
            used_backup = true;
            b
        }
        (Err(primary_reason), Err(backup_reason)) => {
            if !matches!(primary_reason, GptHeaderError::Signature) {
                if matches!(primary_reason, GptHeaderError::Crc { .. }) {
                    diagnostics.push(VolumeDiagnostic::GptHeaderCrcMismatch);
                }
                diagnostics.push(VolumeDiagnostic::PrimaryGptInvalid {
                    reason: primary_reason.to_string(),
                });
                diagnostics.push(VolumeDiagnostic::BackupGptInvalid {
                    reason: backup_reason.to_string(),
                });
            }
            return Ok(None);
        }
    };

    if !chosen.array_crc_ok {
        diagnostics.push(VolumeDiagnostic::GptArrayCrcMismatch);
    }

    let mut partitions = Vec::new();
    for (index, entry) in &chosen.entries {
        if entry.last_lba < entry.first_lba {
            diagnostics.push(VolumeDiagnostic::ZeroLengthPartition { index: *index });
            continue;
        }
        let mut p = Partition::from_lba(
            *index,
            entry.first_lba,
            entry.last_lba,
            sector_size,
            PartitionType::Gpt(entry.type_guid),
        )?;
        p.name = if entry.name.is_empty() {
            None
        } else {
            Some(entry.name.clone())
        };
        p.unique_guid = Some(entry.unique_guid);
        p.flags = gpt_flags(entry.attributes);
        if entry.name_invalid_utf16 {
            diagnostics.push(VolumeDiagnostic::InvalidUtf16PartitionName { index: *index });
        }
        if !chosen.array_crc_ok {
            p.confidence = crate::PartitionConfidence::Medium;
        }
        partitions.push(p);
    }

    Ok(Some(GptScan {
        header: chosen.header,
        partitions,
        diagnostics: Vec::new(),
        used_backup,
    }))
}

fn gpt_flags(attributes: u64) -> PartitionFlags {
    let mut flags = PartitionFlags::empty();
    if attributes & 1 != 0 {
        flags |= PartitionFlags::GPT_REQUIRED;
    }
    if attributes & (1 << 1) != 0 {
        flags |= PartitionFlags::GPT_NO_BLOCK_IO;
    }
    if attributes & (1 << 2) != 0 {
        flags |= PartitionFlags::GPT_LEGACY_BIOS_BOOTABLE;
    }
    if attributes & (1 << 60) != 0 {
        flags |= PartitionFlags::READ_ONLY;
    }
    if attributes & (1 << 61) != 0 {
        flags |= PartitionFlags::SHADOW_COPY;
    }
    if attributes & (1 << 62) != 0 {
        flags |= PartitionFlags::HIDDEN;
    }
    if attributes & (1 << 63) != 0 {
        flags |= PartitionFlags::NO_AUTOMOUNT;
    }
    flags
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builder for synthetic GPT disks used across the crate's tests.

    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    use uuid::Uuid;

    /// A partition to place in a synthetic GPT.
    pub struct Part {
        pub type_guid: Uuid,
        pub first: u64,
        pub last: u64,
        pub name: &'static str,
        pub attributes: u64,
    }

    /// Options for the builder.
    pub struct Layout {
        pub sector_size: u32,
        pub total_sectors: u64,
        pub parts: Vec<Part>,
        pub protective_mbr: bool,
        pub entry_count: u32,
    }

    impl Layout {
        pub fn new(total_sectors: u64, parts: Vec<Part>) -> Self {
            Self {
                sector_size: 512,
                total_sectors,
                parts,
                protective_mbr: true,
                entry_count: 128,
            }
        }
    }

    fn write_entry(buf: &mut [u8], p: &Part) {
        buf[..16].copy_from_slice(&p.type_guid.to_bytes_le());
        buf[16..32].copy_from_slice(&Uuid::new_v4().to_bytes_le());
        buf[32..40].copy_from_slice(&p.first.to_le_bytes());
        buf[40..48].copy_from_slice(&p.last.to_le_bytes());
        buf[48..56].copy_from_slice(&p.attributes.to_le_bytes());
        let mut name = Vec::new();
        for u in p.name.encode_utf16().take(36) {
            name.extend_from_slice(&u.to_le_bytes());
        }
        buf[56..56 + name.len()].copy_from_slice(&name);
    }

    #[allow(clippy::too_many_arguments)]
    fn header(
        sector_size: u32,
        current: u64,
        backup: u64,
        first_usable: u64,
        last_usable: u64,
        guid: Uuid,
        entries_lba: u64,
        entry_count: u32,
        array_crc: u32,
    ) -> Vec<u8> {
        let mut h = vec![0u8; sector_size as usize];
        h[..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&92u32.to_le_bytes());
        h[24..32].copy_from_slice(&current.to_le_bytes());
        h[32..40].copy_from_slice(&backup.to_le_bytes());
        h[40..48].copy_from_slice(&first_usable.to_le_bytes());
        h[48..56].copy_from_slice(&last_usable.to_le_bytes());
        h[56..72].copy_from_slice(&guid.to_bytes_le());
        h[72..80].copy_from_slice(&entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&entry_count.to_le_bytes());
        h[84..88].copy_from_slice(&128u32.to_le_bytes());
        h[88..92].copy_from_slice(&array_crc.to_le_bytes());
        let crc = crc32fast::hash(&h[..92]);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        h
    }

    /// Builds a complete disk image with primary and backup GPT.
    pub fn build(layout: &Layout) -> Vec<u8> {
        let ss = layout.sector_size as u64;
        let total = layout.total_sectors;
        let mut disk = vec![0u8; (total * ss) as usize];
        let array_bytes = layout.entry_count as u64 * 128;
        let array_sectors = array_bytes.div_ceil(ss);
        let mut array = vec![0u8; array_bytes as usize];
        for (i, p) in layout.parts.iter().enumerate() {
            write_entry(&mut array[i * 128..(i + 1) * 128], p);
        }
        let array_crc = crc32fast::hash(&array);
        let guid = Uuid::new_v4();
        let first_usable = 2 + array_sectors;
        let last_usable = total - 2 - array_sectors;
        let backup_lba = total - 1;
        let backup_array_lba = total - 1 - array_sectors;

        if layout.protective_mbr {
            let mut mbr = crate::mbr::testutil::sector(
                &[(0, 0xEE, 1, u32::try_from(total - 1).unwrap_or(u32::MAX))],
                true,
            );
            mbr.resize(ss as usize, 0);
            disk[..ss as usize].copy_from_slice(&mbr);
        }
        let primary = header(
            layout.sector_size,
            1,
            backup_lba,
            first_usable,
            last_usable,
            guid,
            2,
            layout.entry_count,
            array_crc,
        );
        disk[ss as usize..2 * ss as usize].copy_from_slice(&primary);
        disk[2 * ss as usize..2 * ss as usize + array.len()].copy_from_slice(&array);
        let backup = header(
            layout.sector_size,
            backup_lba,
            1,
            first_usable,
            last_usable,
            guid,
            backup_array_lba,
            layout.entry_count,
            array_crc,
        );
        let bo = (backup_lba * ss) as usize;
        disk[bo..bo + ss as usize].copy_from_slice(&backup);
        let bao = (backup_array_lba * ss) as usize;
        disk[bao..bao + array.len()].copy_from_slice(&array);
        disk
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

    use super::testutil::{Layout, Part, build};
    use super::*;
    use crate::model::gpt_types;
    use phoinix_block::{BlockGeometry, MemoryReader};

    fn two_partitions() -> Layout {
        Layout::new(
            8192,
            vec![
                Part {
                    type_guid: gpt_types::EFI_SYSTEM,
                    first: 34,
                    last: 2081,
                    name: "EFI System",
                    attributes: 1,
                },
                Part {
                    type_guid: gpt_types::BASIC_DATA,
                    first: 2082,
                    last: 8000,
                    name: "Données",
                    attributes: 1 << 63,
                },
            ],
        )
    }

    #[test]
    fn parses_valid_gpt() {
        let disk = build(&two_partitions());
        let r = MemoryReader::new(disk);
        let mut diags = Vec::new();
        let scan = scan_gpt(&r, &mut diags).unwrap().unwrap();
        assert!(!scan.used_backup);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(scan.partitions.len(), 2);
        assert_eq!(scan.partitions[0].index, 1);
        assert_eq!(scan.partitions[0].start_lba, 34);
        assert_eq!(scan.partitions[0].end_lba, 2081);
        assert_eq!(scan.partitions[0].length, 2048 * 512);
        assert_eq!(scan.partitions[0].name.as_deref(), Some("EFI System"));
        assert!(
            scan.partitions[0]
                .flags
                .contains(PartitionFlags::GPT_REQUIRED)
        );
        assert_eq!(scan.partitions[1].name.as_deref(), Some("Données"));
        assert!(
            scan.partitions[1]
                .flags
                .contains(PartitionFlags::NO_AUTOMOUNT)
        );
        assert_eq!(
            scan.partitions[1].partition_type,
            PartitionType::Gpt(gpt_types::BASIC_DATA)
        );
    }

    #[test]
    fn corrupt_primary_header_crc_falls_back_to_backup() {
        let mut disk = build(&two_partitions());
        disk[512 + 30] ^= 0xFF; // inside the primary header
        let r = MemoryReader::new(disk);
        let mut diags = Vec::new();
        let scan = scan_gpt(&r, &mut diags).unwrap().unwrap();
        assert!(scan.used_backup);
        assert!(diags.contains(&VolumeDiagnostic::GptHeaderCrcMismatch));
        assert!(diags.contains(&VolumeDiagnostic::BackupGptValid));
        assert_eq!(scan.partitions.len(), 2);
    }

    #[test]
    fn corrupt_primary_array_uses_backup_array() {
        let mut disk = build(&two_partitions());
        disk[1024 + 40] ^= 0x01; // first entry's first_lba in the primary array
        let r = MemoryReader::new(disk);
        let mut diags = Vec::new();
        let scan = scan_gpt(&r, &mut diags).unwrap().unwrap();
        assert!(scan.used_backup);
        assert!(diags.contains(&VolumeDiagnostic::GptArrayCrcMismatch));
        assert_eq!(scan.partitions[0].start_lba, 34);
    }

    #[test]
    fn both_arrays_corrupt_lowers_confidence() {
        let mut disk = build(&two_partitions());
        disk[1024 + 40] ^= 0x01;
        let bao = (8192 - 1 - 32) * 512;
        disk[bao + 40] ^= 0x01;
        let r = MemoryReader::new(disk);
        let mut diags = Vec::new();
        let scan = scan_gpt(&r, &mut diags).unwrap().unwrap();
        assert!(diags.contains(&VolumeDiagnostic::GptArrayCrcMismatch));
        assert_eq!(
            scan.partitions[0].confidence,
            crate::PartitionConfidence::Medium
        );
    }

    #[test]
    fn no_gpt_returns_none_silently() {
        let r = MemoryReader::zeroed(8192 * 512);
        let mut diags = Vec::new();
        assert!(scan_gpt(&r, &mut diags).unwrap().is_none());
        assert!(diags.is_empty());
    }

    #[test]
    fn header_rejects_bad_geometry() {
        let disk = build(&two_partitions());
        let mut sector = disk[512..1024].to_vec();
        // Entry size 100 (not power of two) with recomputed CRC.
        sector[84..88].copy_from_slice(&100u32.to_le_bytes());
        sector[16..20].copy_from_slice(&[0; 4]);
        let crc = crc32fast::hash(&sector[..92]);
        sector[16..20].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            GptHeader::parse(&sector, 1, 512, 8192),
            Err(GptHeaderError::EntryGeometry { .. })
        ));
        assert!(matches!(
            GptHeader::parse(&sector, 5, 512, 8192),
            Err(GptHeaderError::Location {
                claimed: 1,
                actual: 5
            })
        ));
        assert!(matches!(
            GptHeader::parse(&sector[..50], 1, 512, 8192),
            Err(GptHeaderError::Truncated)
        ));
        assert!(matches!(
            GptHeader::parse(&[0u8; 512], 1, 512, 8192),
            Err(GptHeaderError::Signature)
        ));
    }

    #[test]
    fn four_k_sector_gpt() {
        let mut layout = two_partitions();
        layout.sector_size = 4096;
        layout.total_sectors = 2048;
        layout.parts[0].last = 600;
        layout.parts[1].first = 601;
        layout.parts[1].last = 2000;
        let disk = build(&layout);
        let r = MemoryReader::with_geometry(disk, BlockGeometry::SECTOR_4K);
        let mut diags = Vec::new();
        let scan = scan_gpt(&r, &mut diags).unwrap().unwrap();
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(scan.partitions[0].start_offset, 34 * 4096);
        assert_eq!(scan.partitions[1].end_lba, 2000);
    }

    #[test]
    fn invalid_utf16_name_is_diagnosed() {
        let mut disk = build(&two_partitions());
        // Unpaired high surrogate at the start of entry 1's name.
        disk[1024 + 56..1024 + 60].copy_from_slice(&[0x00, 0xD8, b'x', 0x00]);
        let array = disk[1024..1024 + 128 * 128].to_vec();
        let crc = crc32fast::hash(&array);
        disk[512 + 88..512 + 92].copy_from_slice(&crc.to_le_bytes());
        disk[512 + 16..512 + 20].copy_from_slice(&[0; 4]);
        let hcrc = crc32fast::hash(&disk[512..512 + 92]);
        disk[512 + 16..512 + 20].copy_from_slice(&hcrc.to_le_bytes());
        let r = MemoryReader::new(disk);
        let mut diags = Vec::new();
        let scan = scan_gpt(&r, &mut diags).unwrap().unwrap();
        assert!(
            diags.contains(&VolumeDiagnostic::InvalidUtf16PartitionName { index: 1 }),
            "{diags:?}"
        );
        assert!(
            scan.partitions[0]
                .name
                .as_deref()
                .unwrap()
                .contains('\u{FFFD}')
        );
    }
}
