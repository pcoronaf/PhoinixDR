//! Directory entry sets.

use phoinix_core::bytes::{ByteView, utf16le_to_string_lossy};
use phoinix_core::fmt::dos_datetime_to_unix;
use serde::{Deserialize, Serialize};

/// Size of one entry.
pub const ENTRY_SIZE: usize = 32;
/// Entry types (with the in-use bit set).
pub const TYPE_BITMAP: u8 = 0x81;
/// Up-case table.
pub const TYPE_UPCASE: u8 = 0x82;
/// Volume label.
pub const TYPE_LABEL: u8 = 0x83;
/// File.
pub const TYPE_FILE: u8 = 0x85;
/// Stream extension.
pub const TYPE_STREAM: u8 = 0xC0;
/// File name.
pub const TYPE_NAME: u8 = 0xC1;

/// exFAT file attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExfatAttributes(pub u16);

impl ExfatAttributes {
    /// Directory bit.
    pub const DIRECTORY: u16 = 0x10;

    /// Whether the entry is a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.0 & Self::DIRECTORY != 0
    }
}

/// Stream extension flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamFlags(pub u8);

impl StreamFlags {
    /// Whether allocation is possible (bit 0).
    #[must_use]
    pub const fn allocation_possible(&self) -> bool {
        self.0 & 0x01 != 0
    }

    /// Whether the clusters are contiguous without a FAT chain (bit 1).
    #[must_use]
    pub const fn no_fat_chain(&self) -> bool {
        self.0 & 0x02 != 0
    }
}

/// A file or directory entry set (File + Stream Extension + Name entries).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntrySet {
    /// Volume byte offset of the File entry.
    pub entry_offset: u64,
    /// Whether the set is deleted (in-use bit clear).
    pub deleted: bool,
    /// Number of secondary entries declared.
    pub secondary_count: u8,
    /// Whether the set checksum matched (computed with the in-use bits
    /// restored for deleted sets).
    pub checksum_ok: bool,
    /// Attributes.
    pub attributes: ExfatAttributes,
    /// Stream flags.
    pub flags: StreamFlags,
    /// Name.
    pub name: String,
    /// Declared name length in UTF-16 units.
    pub name_length: u8,
    /// Valid data length.
    pub valid_data_length: u64,
    /// First cluster (zero when no clusters are allocated).
    pub first_cluster: u32,
    /// Data length.
    pub data_length: u64,
    /// Creation time (Unix seconds).
    pub created: Option<i64>,
    /// Modification time.
    pub modified: Option<i64>,
    /// Access time.
    pub accessed: Option<i64>,
}

impl EntrySet {
    /// Whether the set is a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.attributes.is_directory()
    }
}

/// Non-file primary entries of interest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpecialEntry {
    /// Allocation bitmap (first cluster, length).
    Bitmap {
        /// First cluster.
        first_cluster: u32,
        /// Length in bytes.
        length: u64,
    },
    /// Volume label.
    Label(String),
    /// Up-case table.
    UpCase {
        /// First cluster.
        first_cluster: u32,
        /// Length in bytes.
        length: u64,
    },
}

/// Everything found in one directory.
#[derive(Debug, Clone, Default)]
pub struct Directory {
    /// File and directory entry sets, in order.
    pub entries: Vec<EntrySet>,
    /// Special entries (root directory only).
    pub specials: Vec<SpecialEntry>,
}

/// exFAT timestamp: DOS date/time plus 10 ms and UTC offset.
fn exfat_timestamp(raw: u32, ten_ms: u8, utc_offset: u8) -> Option<i64> {
    let time = (raw & 0xFFFF) as u16;
    let date = (raw >> 16) as u16;
    let mut secs = dos_datetime_to_unix(date, time, 0)?;
    secs += i64::from(ten_ms / 100);
    if utc_offset & 0x80 != 0 {
        // Seven-bit signed offset in 15-minute units; stored time is local.
        let units = i64::from(i8::from_le_bytes([
            (utc_offset & 0x7F) | if utc_offset & 0x40 != 0 { 0x80 } else { 0 }
        ]));
        secs -= units * 15 * 60;
    }
    Some(secs)
}

/// Entry-set checksum over `bytes` (all entries of the set), skipping the
/// checksum field of the primary entry. `deleted` restores the in-use bits.
#[must_use]
pub fn entry_set_checksum(bytes: &[u8], deleted: bool) -> u16 {
    let mut sum: u16 = 0;
    for (i, b) in bytes.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        let mut v = *b;
        if deleted && i % ENTRY_SIZE == 0 {
            v |= 0x80;
        }
        sum = sum.rotate_right(1).wrapping_add(u16::from(v));
    }
    sum
}

/// Parses a directory region. `base_offset` is the volume byte offset of
/// `bytes[0]`.
#[must_use]
pub fn parse_directory(bytes: &[u8], base_offset: u64) -> Directory {
    let view = ByteView::new(bytes);
    let mut dir = Directory::default();
    let mut pos = 0usize;
    while pos + ENTRY_SIZE <= bytes.len() {
        let Some(e) = view.sub(pos, ENTRY_SIZE) else {
            break;
        };
        let t = e.u8(0).unwrap_or(0);
        if t == 0 {
            break;
        }
        let kind = t & 0x7F;
        let in_use = t & 0x80 != 0;
        match kind {
            0x01 if in_use => {
                dir.specials.push(SpecialEntry::Bitmap {
                    first_cluster: e.u32_le(20).unwrap_or(0),
                    length: e.u64_le(24).unwrap_or(0),
                });
                pos += ENTRY_SIZE;
            }
            0x02 if in_use => {
                dir.specials.push(SpecialEntry::UpCase {
                    first_cluster: e.u32_le(20).unwrap_or(0),
                    length: e.u64_le(24).unwrap_or(0),
                });
                pos += ENTRY_SIZE;
            }
            0x03 if in_use => {
                let len = usize::from(e.u8(1).unwrap_or(0)).min(11);
                dir.specials
                    .push(SpecialEntry::Label(utf16le_to_string_lossy(
                        e.slice(2, len * 2).unwrap_or(&[]),
                    )));
                pos += ENTRY_SIZE;
            }
            0x05 => {
                let secondary = e.u8(1).unwrap_or(0);
                let set_len = ENTRY_SIZE * (1 + usize::from(secondary));
                let Some(set) = view.sub(pos, set_len) else {
                    // Truncated set at the end of the region.
                    pos += ENTRY_SIZE;
                    continue;
                };
                let deleted = !in_use;
                if let Some(entry) = parse_set(set, base_offset.saturating_add(pos as u64), deleted)
                {
                    dir.entries.push(entry);
                }
                pos += set_len;
            }
            _ => pos += ENTRY_SIZE,
        }
    }
    dir
}

fn parse_set(set: ByteView<'_>, entry_offset: u64, deleted: bool) -> Option<EntrySet> {
    let secondary_count = set.u8(1)?;
    let stored_checksum = set.u16_le(2)?;
    let attributes = ExfatAttributes(set.u16_le(4)?);
    let created = exfat_timestamp(set.u32_le(8)?, set.u8(20)?, set.u8(22)?);
    let modified = exfat_timestamp(set.u32_le(12)?, set.u8(21)?, set.u8(23)?);
    let accessed = exfat_timestamp(set.u32_le(16)?, 0, set.u8(24)?);
    let checksum_ok = entry_set_checksum(set.as_slice(), deleted) == stored_checksum;

    let mut flags = StreamFlags(0);
    let mut name_length = 0u8;
    let mut valid_data_length = 0u64;
    let mut first_cluster = 0u32;
    let mut data_length = 0u64;
    let mut name_units: Vec<u8> = Vec::new();
    let mut have_stream = false;
    for i in 1..=usize::from(secondary_count) {
        let e = set.sub(i * ENTRY_SIZE, ENTRY_SIZE)?;
        let kind = e.u8(0)? & 0x7F;
        match kind {
            0x40 if !have_stream => {
                have_stream = true;
                flags = StreamFlags(e.u8(1)?);
                name_length = e.u8(3)?;
                valid_data_length = e.u64_le(8)?;
                first_cluster = e.u32_le(20)?;
                data_length = e.u64_le(24)?;
            }
            0x41 => {
                name_units.extend_from_slice(e.slice(2, 30)?);
            }
            _ => {}
        }
    }
    if !have_stream {
        return None;
    }
    let take = usize::from(name_length)
        .saturating_mul(2)
        .min(name_units.len());
    let name = utf16le_to_string_lossy(name_units.get(..take).unwrap_or(&[]));
    Some(EntrySet {
        entry_offset,
        deleted,
        secondary_count,
        checksum_ok,
        attributes,
        flags,
        name,
        name_length,
        valid_data_length,
        first_cluster,
        data_length,
        created,
        modified,
        accessed,
    })
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builders for entry sets.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    use super::{ENTRY_SIZE, entry_set_checksum};

    pub fn file_set(
        name: &str,
        attributes: u16,
        flags: u8,
        first_cluster: u32,
        length: u64,
        deleted: bool,
    ) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let name_entries = units.len().div_ceil(15).max(1);
        let secondary = 1 + name_entries;
        let mut set = vec![0u8; ENTRY_SIZE * (1 + secondary)];
        set[0] = 0x85;
        set[1] = secondary as u8;
        set[4..6].copy_from_slice(&attributes.to_le_bytes());
        let ts: u32 = ((46u32 << 9 | 9 << 5 | 5) << 16) | (1 << 11 | 38 << 5);
        set[8..12].copy_from_slice(&ts.to_le_bytes());
        set[12..16].copy_from_slice(&ts.to_le_bytes());
        set[16..20].copy_from_slice(&ts.to_le_bytes());
        let s = &mut set[ENTRY_SIZE..2 * ENTRY_SIZE];
        s[0] = 0xC0;
        s[1] = flags;
        s[3] = units.len() as u8;
        s[8..16].copy_from_slice(&length.to_le_bytes());
        s[20..24].copy_from_slice(&first_cluster.to_le_bytes());
        s[24..32].copy_from_slice(&length.to_le_bytes());
        for (i, chunk) in units.chunks(15).enumerate() {
            let e = &mut set[(2 + i) * ENTRY_SIZE..(3 + i) * ENTRY_SIZE];
            e[0] = 0xC1;
            for (j, u) in chunk.iter().enumerate() {
                e[2 + j * 2..4 + j * 2].copy_from_slice(&u.to_le_bytes());
            }
        }
        let checksum = entry_set_checksum(&set, false);
        set[2..4].copy_from_slice(&checksum.to_le_bytes());
        if deleted {
            for i in 0..=secondary {
                set[i * ENTRY_SIZE] &= 0x7F;
            }
        }
        set
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

    use super::testutil::file_set;
    use super::*;

    #[test]
    fn parses_sets_and_deleted_sets() {
        let mut dir = Vec::new();
        let mut label = vec![0u8; 32];
        label[0] = 0x83;
        label[1] = 3;
        label[2..8].copy_from_slice(&[b'V', 0, b'O', 0, b'L', 0]);
        dir.extend(label);
        let mut bm = vec![0u8; 32];
        bm[0] = 0x81;
        bm[20..24].copy_from_slice(&2u32.to_le_bytes());
        bm[24..32].copy_from_slice(&1984u64.to_le_bytes());
        dir.extend(bm);
        dir.extend(file_set("docs", 0x10, 0x03, 6, 4096, false));
        dir.extend(file_set(
            "a very long file name that spans entries.txt",
            0x20,
            0x03,
            13,
            4,
            true,
        ));
        dir.extend(vec![0u8; 32]);
        let d = parse_directory(&dir, 5000);
        assert_eq!(d.specials.len(), 2);
        assert_eq!(d.specials[0], SpecialEntry::Label("VOL".into()));
        assert_eq!(d.entries.len(), 2);
        assert_eq!(d.entries[0].name, "docs");
        assert!(d.entries[0].is_directory());
        assert!(d.entries[0].checksum_ok);
        assert!(d.entries[0].flags.no_fat_chain());
        assert_eq!(d.entries[0].entry_offset, 5064);
        let del = &d.entries[1];
        assert!(del.deleted);
        assert!(
            del.checksum_ok,
            "deleted set checksum verified with in-use bits restored"
        );
        assert_eq!(del.name, "a very long file name that spans entries.txt");
        assert_eq!(del.first_cluster, 13);
        assert_eq!(del.data_length, 4);
        assert_eq!(
            del.modified
                .map(|s| phoinix_core::fmt::iso8601_utc(s, 0))
                .as_deref(),
            Some("2026-09-05T01:38:00.000000Z")
        );
    }

    #[test]
    fn corrupt_checksum_is_flagged_not_fatal() {
        let mut set = file_set("x.bin", 0x20, 0x03, 9, 10, false);
        set[40] ^= 0xFF;
        let d = parse_directory(&set, 0);
        assert_eq!(d.entries.len(), 1);
        assert!(!d.entries[0].checksum_ok);
    }
}
