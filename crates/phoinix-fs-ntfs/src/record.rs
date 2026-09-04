//! MFT FILE records.

use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::attribute::AttributeIter;
use crate::fixup::apply_fixup;

/// Reference to an MFT record: 48-bit record number plus 16-bit sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileReference {
    /// Record number.
    pub record: u64,
    /// Sequence number of the record at the time the reference was made.
    pub sequence: u16,
}

impl FileReference {
    /// Decodes the packed 64-bit on-disk form.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self {
            record: raw & 0x0000_FFFF_FFFF_FFFF,
            sequence: (raw >> 48) as u16,
        }
    }

    /// The packed 64-bit form.
    #[must_use]
    pub const fn to_raw(self) -> u64 {
        ((self.sequence as u64) << 48) | (self.record & 0x0000_FFFF_FFFF_FFFF)
    }
}

impl std::fmt::Display for FileReference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}-{}", self.record, self.sequence)
    }
}

/// Record flag: the record is in use.
pub const FLAG_IN_USE: u16 = 0x0001;
/// Record flag: the record describes a directory.
pub const FLAG_DIRECTORY: u16 = 0x0002;

/// Header of a FILE record (after fixup).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecordHeader {
    /// `$LogFile` sequence number.
    pub log_sequence_number: u64,
    /// Sequence number; incremented each time the record is reused.
    pub sequence_number: u16,
    /// Hard link count.
    pub hard_link_count: u16,
    /// Offset of the first attribute.
    pub first_attribute_offset: u16,
    /// Raw flags.
    pub flags: u16,
    /// Bytes of the record in use.
    pub used_size: u32,
    /// Allocated size of the record.
    pub allocated_size: u32,
    /// Base record for extension records (zero for base records).
    pub base_reference: FileReference,
    /// Next attribute identifier to assign.
    pub next_attribute_id: u16,
    /// Record number stored in the header (NTFS 3.1+), if present.
    pub stored_record_number: Option<u32>,
}

impl FileRecordHeader {
    /// Whether the record is in use (not deleted).
    #[must_use]
    pub const fn in_use(&self) -> bool {
        self.flags & FLAG_IN_USE != 0
    }

    /// Whether the record is a directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.flags & FLAG_DIRECTORY != 0
    }

    /// Whether this is a base record rather than an extension.
    #[must_use]
    pub const fn is_base(&self) -> bool {
        self.base_reference.record == 0 && self.base_reference.sequence == 0
    }
}

/// A fixed-up FILE record ready for attribute parsing.
#[derive(Debug, Clone)]
pub struct FileRecord {
    number: u64,
    header: FileRecordHeader,
    data: Vec<u8>,
}

impl FileRecord {
    /// Parses a raw record of `record_size` bytes as read from the MFT,
    /// verifying the signature and applying the update sequence fixup.
    ///
    /// `stride` is the volume's bytes-per-sector.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::InvalidRecord`] or [`NtfsError::FixupMismatch`].
    pub fn parse(number: u64, mut data: Vec<u8>, stride: usize) -> Result<Self, NtfsError> {
        let invalid = |reason: String| NtfsError::InvalidRecord {
            record: number,
            reason,
        };
        let view = ByteView::new(&data);
        let signature = view
            .array::<4>(0)
            .ok_or_else(|| invalid("record shorter than a header".into()))?;
        match &signature {
            b"FILE" => {}
            b"BAAD" => return Err(invalid("record is marked BAAD by chkdsk".into())),
            [0, 0, 0, 0] => return Err(invalid("record is empty".into())),
            other => return Err(invalid(format!("bad signature {other:02x?}"))),
        }
        let usa_offset = view.u16_le(4).ok_or_else(|| invalid("truncated".into()))?;
        let usa_count = view.u16_le(6).ok_or_else(|| invalid("truncated".into()))?;
        apply_fixup(&mut data, usa_offset, usa_count, stride, number)?;

        let view = ByteView::new(&data);
        let read16 = |o: usize| {
            view.u16_le(o)
                .ok_or_else(|| invalid("truncated header".into()))
        };
        let read32 = |o: usize| {
            view.u32_le(o)
                .ok_or_else(|| invalid("truncated header".into()))
        };
        let header = FileRecordHeader {
            log_sequence_number: view
                .u64_le(8)
                .ok_or_else(|| invalid("truncated header".into()))?,
            sequence_number: read16(0x10)?,
            hard_link_count: read16(0x12)?,
            first_attribute_offset: read16(0x14)?,
            flags: read16(0x16)?,
            used_size: read32(0x18)?,
            allocated_size: read32(0x1C)?,
            base_reference: FileReference::from_raw(
                view.u64_le(0x20)
                    .ok_or_else(|| invalid("truncated header".into()))?,
            ),
            next_attribute_id: read16(0x28)?,
            stored_record_number: if usa_offset >= 0x30 {
                view.u32_le(0x2C)
            } else {
                None
            },
        };
        let len = u32::try_from(data.len()).map_err(|_| NtfsError::Overflow)?;
        if header.used_size > len || header.used_size < u32::from(header.first_attribute_offset) {
            return Err(invalid(format!(
                "used size {} is inconsistent with record size {len}",
                header.used_size
            )));
        }
        if usize::from(header.first_attribute_offset)
            < usize::from(usa_offset) + usize::from(usa_count) * 2
        {
            return Err(invalid(
                "first attribute overlaps the update sequence array".into(),
            ));
        }
        if let Some(stored) = header.stored_record_number
            && u64::from(stored) != number
        {
            return Err(invalid(format!(
                "header record number {stored} does not match position {number}"
            )));
        }
        Ok(Self {
            number,
            header,
            data,
        })
    }

    /// MFT record number.
    #[must_use]
    pub const fn number(&self) -> u64 {
        self.number
    }

    /// The header.
    #[must_use]
    pub const fn header(&self) -> &FileRecordHeader {
        &self.header
    }

    /// Reference to this record (number plus current sequence).
    #[must_use]
    pub const fn reference(&self) -> FileReference {
        FileReference {
            record: self.number,
            sequence: self.header.sequence_number,
        }
    }

    /// The fixed-up bytes.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Iterates the attributes in this record.
    #[must_use]
    pub fn attributes(&self) -> AttributeIter<'_> {
        AttributeIter::new(
            self.number,
            &self.data,
            usize::from(self.header.first_attribute_offset),
            self.header.used_size as usize,
        )
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
    use crate::fixup::testutil::protect;

    pub(crate) fn blank_record(number: u64, flags: u16) -> Vec<u8> {
        let mut r = vec![0u8; 1024];
        r[..4].copy_from_slice(b"FILE");
        r[0x10..0x12].copy_from_slice(&3u16.to_le_bytes()); // sequence
        r[0x12..0x14].copy_from_slice(&1u16.to_le_bytes()); // links
        r[0x14..0x16].copy_from_slice(&0x38u16.to_le_bytes()); // first attribute
        r[0x16..0x18].copy_from_slice(&flags.to_le_bytes());
        r[0x18..0x1C].copy_from_slice(&0x40u32.to_le_bytes()); // used
        r[0x1C..0x20].copy_from_slice(&1024u32.to_le_bytes()); // allocated
        r[0x2C..0x30].copy_from_slice(&(number as u32).to_le_bytes());
        r[0x38..0x3C].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // end marker
        protect(&mut r, 0x30, 512, 0x0102);
        r
    }

    #[test]
    fn parses_header() {
        let rec = FileRecord::parse(5, blank_record(5, FLAG_IN_USE | FLAG_DIRECTORY), 512).unwrap();
        assert!(rec.header().in_use());
        assert!(rec.header().is_directory());
        assert!(rec.header().is_base());
        assert_eq!(
            rec.reference(),
            FileReference {
                record: 5,
                sequence: 3
            }
        );
        assert_eq!(rec.header().stored_record_number, Some(5));
        assert_eq!(rec.attributes().count(), 0);
    }

    #[test]
    fn rejects_bad_signature_and_mismatched_number() {
        let mut r = blank_record(5, 1);
        r[..4].copy_from_slice(b"BAAD");
        assert!(matches!(
            FileRecord::parse(5, r, 512),
            Err(NtfsError::InvalidRecord { .. })
        ));
        assert!(matches!(
            FileRecord::parse(6, blank_record(5, 1), 512),
            Err(NtfsError::InvalidRecord { .. })
        ));
        assert!(matches!(
            FileRecord::parse(0, vec![0u8; 1024], 512),
            Err(NtfsError::InvalidRecord { .. })
        ));
        let mut r = blank_record(5, 1);
        r[1022] ^= 1;
        assert!(matches!(
            FileRecord::parse(5, r, 512),
            Err(NtfsError::FixupMismatch { .. })
        ));
    }

    #[test]
    fn file_reference_packing() {
        let r = FileReference::from_raw(0x0005_0000_0000_002A);
        assert_eq!(
            r,
            FileReference {
                record: 42,
                sequence: 5
            }
        );
        assert_eq!(r.to_raw(), 0x0005_0000_0000_002A);
        assert_eq!(r.to_string(), "42-5");
    }
}
