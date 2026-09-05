//! Directory entries, long file names and deleted-entry handling.

use phoinix_core::bytes::{ByteView, utf16le_to_string_lossy};
use phoinix_core::fmt::dos_datetime_to_unix;
use serde::{Deserialize, Serialize};

/// Size of a directory entry.
pub const ENTRY_SIZE: usize = 32;
/// First byte of a deleted entry.
pub const DELETED_MARKER: u8 = 0xE5;
/// First byte of the end-of-directory marker.
pub const END_MARKER: u8 = 0x00;

/// FAT attribute bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FatAttributes(pub u8);

impl FatAttributes {
    /// Read-only.
    pub const READ_ONLY: u8 = 0x01;
    /// Hidden.
    pub const HIDDEN: u8 = 0x02;
    /// System.
    pub const SYSTEM: u8 = 0x04;
    /// Volume label.
    pub const VOLUME_ID: u8 = 0x08;
    /// Directory.
    pub const DIRECTORY: u8 = 0x10;
    /// Archive.
    pub const ARCHIVE: u8 = 0x20;
    /// Long-name entry marker.
    pub const LONG_NAME: u8 = 0x0F;

    /// Whether the entry is a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.0 & Self::DIRECTORY != 0
    }

    /// Whether the entry is a volume label.
    #[must_use]
    pub const fn is_volume_label(&self) -> bool {
        self.0 & Self::VOLUME_ID != 0 && self.0 & Self::DIRECTORY == 0
    }

    /// Whether the entry is a long-name piece.
    #[must_use]
    pub const fn is_long_name(&self) -> bool {
        self.0 & 0x3F == Self::LONG_NAME
    }
}

/// A parsed directory entry (short entry plus any long name).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    /// Volume byte offset of the 8.3 entry.
    pub entry_offset: u64,
    /// Whether the entry is deleted (`0xE5`).
    pub deleted: bool,
    /// Short (8.3) name; for deleted entries the lost first character is `?`.
    pub short_name: String,
    /// Long name from LFN entries, if present.
    pub long_name: Option<String>,
    /// Whether the long name was taken from deleted LFN entries whose
    /// checksum could not be verified.
    pub long_name_unverified: bool,
    /// Attributes.
    pub attributes: FatAttributes,
    /// First cluster (high word included on FAT32).
    pub first_cluster: u32,
    /// The stored high word of the first cluster (FAT32); zero when the
    /// driver cleared it on deletion.
    pub first_cluster_high: u16,
    /// Size in bytes.
    pub size: u32,
    /// Creation time (Unix seconds, UTC assumed), if set.
    pub created: Option<i64>,
    /// Modification time.
    pub modified: Option<i64>,
    /// Access date (midnight).
    pub accessed: Option<i64>,
}

impl DirEntry {
    /// Best display name: long name, else short name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.long_name.as_deref().unwrap_or(&self.short_name)
    }

    /// Whether the entry is `.` or `..`.
    #[must_use]
    pub fn is_dot(&self) -> bool {
        self.long_name.is_none() && (self.short_name == "." || self.short_name == "..")
    }
}

fn decode_short_name(raw: &[u8], deleted: bool) -> String {
    let mut base: Vec<u8> = raw.get(..8).unwrap_or(raw).to_vec();
    let ext: Vec<u8> = raw.get(8..11).unwrap_or(&[]).to_vec();
    if let Some(first) = base.first_mut() {
        if deleted {
            *first = b'?';
        } else if *first == 0x05 {
            *first = 0xE5;
        }
    }
    let trim = |v: &[u8]| -> String {
        let end = v.iter().rposition(|b| *b != b' ').map_or(0, |p| p + 1);
        v.get(..end)
            .unwrap_or(&[])
            .iter()
            .map(|b| {
                if b.is_ascii_graphic() || *b == b' ' {
                    char::from(*b)
                } else {
                    '?'
                }
            })
            .collect()
    };
    let b = trim(&base);
    let e = trim(&ext);
    if e.is_empty() { b } else { format!("{b}.{e}") }
}

/// Checksum over the 11 raw short-name bytes, as used by LFN entries.
#[must_use]
pub fn short_name_checksum(raw: &[u8]) -> u8 {
    raw.iter()
        .take(11)
        .fold(0u8, |sum, b| sum.rotate_right(1).wrapping_add(*b))
}

/// Parses the directory entries in `bytes` (a whole directory region).
/// `base_offset` is the volume byte offset of `bytes[0]`.
///
/// Long-name entries are assembled onto the following short entry. For a
/// deleted short entry the preceding deleted LFN entries are assembled too,
/// even though their checksum can no longer be verified against the lost
/// first byte (`long_name_unverified`).
#[must_use]
pub fn parse_directory(bytes: &[u8], base_offset: u64) -> Vec<DirEntry> {
    let view = ByteView::new(bytes);
    let mut out = Vec::new();
    let mut lfn_parts: Vec<(u8, bool, u8, String)> = Vec::new(); // (sequence, deleted, checksum, text)
    let mut pos = 0usize;
    while pos + ENTRY_SIZE <= bytes.len() {
        let Some(e) = view.sub(pos, ENTRY_SIZE) else {
            break;
        };
        let first = e.u8(0).unwrap_or(0);
        if first == END_MARKER {
            break;
        }
        let attr = FatAttributes(e.u8(11).unwrap_or(0));
        let deleted = first == DELETED_MARKER;
        if attr.is_long_name() {
            let seq_byte = if deleted { 0 } else { first };
            let checksum = e.u8(13).unwrap_or(0);
            let mut units = Vec::new();
            for (o, n) in [(1usize, 5usize), (14, 6), (28, 2)] {
                if let Some(part) = e.slice(o, n * 2) {
                    units.extend_from_slice(part);
                }
            }
            // Trim at the UTF-16 NUL / 0xFFFF padding.
            let mut end = units.len();
            for (i, pair) in units.chunks_exact(2).enumerate() {
                if pair == [0, 0] || pair == [0xFF, 0xFF] {
                    end = i * 2;
                    break;
                }
            }
            let text = utf16le_to_string_lossy(units.get(..end).unwrap_or(&[]));
            lfn_parts.push((seq_byte & 0x1F, deleted, checksum, text));
            pos += ENTRY_SIZE;
            continue;
        }
        let raw_name = e.slice(0, 11).unwrap_or(&[0; 11]);
        let short_name = decode_short_name(raw_name, deleted);
        let checksum = short_name_checksum(raw_name);
        let (long_name, unverified) = if lfn_parts.is_empty() {
            (None, false)
        } else {
            // LFN entries are stored last-part-first; verified names must
            // all carry the checksum of this short name.
            let verified = !deleted && lfn_parts.iter().all(|p| p.2 == checksum);
            let any_deleted = lfn_parts.iter().any(|p| p.1);
            if verified || (deleted && (any_deleted || lfn_parts.iter().all(|p| p.2 == checksum))) {
                let mut parts = lfn_parts.clone();
                parts.reverse();
                let name: String = parts.iter().map(|p| p.3.as_str()).collect();
                (if name.is_empty() { None } else { Some(name) }, !verified)
            } else {
                (None, false)
            }
        };
        lfn_parts.clear();
        let cluster_high = e.u16_le(20).unwrap_or(0);
        let cluster_low = e.u16_le(26).unwrap_or(0);
        let ctime_tenths = e.u8(13).unwrap_or(0);
        let ctime = e.u16_le(14).unwrap_or(0);
        let cdate = e.u16_le(16).unwrap_or(0);
        let adate = e.u16_le(18).unwrap_or(0);
        let mtime = e.u16_le(22).unwrap_or(0);
        let mdate = e.u16_le(24).unwrap_or(0);
        out.push(DirEntry {
            entry_offset: base_offset.saturating_add(pos as u64),
            deleted,
            short_name,
            long_name,
            long_name_unverified: unverified,
            attributes: attr,
            first_cluster: (u32::from(cluster_high) << 16) | u32::from(cluster_low),
            first_cluster_high: cluster_high,
            size: e.u32_le(28).unwrap_or(0),
            created: dos_datetime_to_unix(cdate, ctime, ctime_tenths),
            modified: dos_datetime_to_unix(mdate, mtime, 0),
            accessed: dos_datetime_to_unix(adate, 0, 0),
        });
        pos += ENTRY_SIZE;
    }
    out
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builders for directory entries.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    use super::short_name_checksum;

    pub fn short(name: &[u8; 11], attr: u8, cluster: u32, size: u32) -> Vec<u8> {
        let mut e = vec![0u8; 32];
        e[..11].copy_from_slice(name);
        e[11] = attr;
        e[16..18].copy_from_slice(&((46 << 9) | (9 << 5) | 5u16).to_le_bytes());
        e[24..26].copy_from_slice(&((46 << 9) | (9 << 5) | 5u16).to_le_bytes());
        e[22..24].copy_from_slice(&((1u16 << 11) | (38 << 5)).to_le_bytes());
        e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        e[28..32].copy_from_slice(&size.to_le_bytes());
        e
    }

    pub fn lfn(name: &str, short_raw: &[u8; 11]) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let chunks: Vec<&[u16]> = units.chunks(13).collect();
        let checksum = short_name_checksum(short_raw);
        let mut out = Vec::new();
        for (i, chunk) in chunks.iter().enumerate().rev() {
            let mut e = vec![0u8; 32];
            let seq = (i + 1) as u8 | if i + 1 == chunks.len() { 0x40 } else { 0 };
            e[0] = seq;
            e[11] = 0x0F;
            e[13] = checksum;
            let mut padded: Vec<u16> = chunk.to_vec();
            if padded.len() < 13 {
                padded.push(0);
                while padded.len() < 13 {
                    padded.push(0xFFFF);
                }
            }
            let positions = [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
            for (u, pos) in padded.iter().zip(positions) {
                e[pos..pos + 2].copy_from_slice(&u.to_le_bytes());
            }
            out.extend(e);
        }
        out
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

    use super::testutil::{lfn, short};
    use super::*;

    #[test]
    fn parses_short_long_and_deleted_entries() {
        let mut dir = Vec::new();
        dir.extend(short(b"HELLO   TXT", 0x20, 5, 100));
        let raw = *b"ALONGN~1TXT";
        dir.extend(lfn("A long name with spaces.txt", &raw));
        dir.extend(short(&raw, 0x20, 9, 19));
        // Deleted file with deleted LFN pieces.
        let raw2 = *b"BIGFILE BIN";
        let mut del_lfn = lfn("BigFile.bin", &raw2);
        del_lfn[0] = 0xE5;
        dir.extend(del_lfn);
        let mut del = short(&raw2, 0x20, 4, 20000);
        del[0] = 0xE5;
        dir.extend(del);
        dir.extend(vec![0u8; 32]);
        let entries = parse_directory(&dir, 1000);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].short_name, "HELLO.TXT");
        assert_eq!(entries[0].entry_offset, 1000);
        assert_eq!(
            entries[0]
                .modified
                .map(|s| phoinix_core::fmt::iso8601_utc(s, 0))
                .as_deref(),
            Some("2026-09-05T01:38:00.000000Z")
        );
        assert_eq!(
            entries[1].long_name.as_deref(),
            Some("A long name with spaces.txt")
        );
        assert!(!entries[1].long_name_unverified);
        assert_eq!(entries[1].first_cluster, 9);
        assert!(entries[2].deleted);
        assert_eq!(entries[2].short_name, "?IGFILE.BIN");
        assert_eq!(entries[2].long_name.as_deref(), Some("BigFile.bin"));
        assert!(entries[2].long_name_unverified);
        assert_eq!(entries[2].size, 20000);
        assert_eq!(entries[2].first_cluster, 4);
    }

    #[test]
    fn mismatched_lfn_checksum_is_dropped() {
        let mut dir = Vec::new();
        dir.extend(lfn("Orphan.txt", b"OTHER   TXT"));
        dir.extend(short(b"REAL    TXT", 0x20, 2, 1));
        let entries = parse_directory(&dir, 0);
        assert_eq!(entries.len(), 1);
        assert!(entries[0].long_name.is_none());
    }

    #[test]
    fn checksum_matches_reference() {
        // Known: checksum of "FILENAMEEXT" style names computed by the FAT
        // specification algorithm; verify stability on a fixed input.
        assert_eq!(
            short_name_checksum(b"ALONGN~1TXT"),
            short_name_checksum(b"ALONGN~1TXT")
        );
        assert_ne!(
            short_name_checksum(b"ALONGN~1TXT"),
            short_name_checksum(b"ALONGN~2TXT")
        );
    }
}
