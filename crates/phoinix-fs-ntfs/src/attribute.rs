//! Attribute headers, resident and non-resident bodies.

use phoinix_core::bytes::{ByteView, utf16le_to_string_lossy};
use serde::{Deserialize, Serialize};

use crate::NtfsError;

/// End-of-attributes marker.
pub const END_MARKER: u32 = 0xFFFF_FFFF;

/// Attribute flag: compressed (compression unit in the non-resident header).
pub const FLAG_COMPRESSED: u16 = 0x0001;
/// Mask of compression-method bits.
pub const FLAG_COMPRESSION_MASK: u16 = 0x00FF;
/// Attribute flag: encrypted (EFS).
pub const FLAG_ENCRYPTED: u16 = 0x4000;
/// Attribute flag: sparse.
pub const FLAG_SPARSE: u16 = 0x8000;

/// NTFS attribute type codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AttributeType {
    /// `$STANDARD_INFORMATION`.
    StandardInformation,
    /// `$ATTRIBUTE_LIST`.
    AttributeList,
    /// `$FILE_NAME`.
    FileName,
    /// `$OBJECT_ID`.
    ObjectId,
    /// `$SECURITY_DESCRIPTOR`.
    SecurityDescriptor,
    /// `$VOLUME_NAME`.
    VolumeName,
    /// `$VOLUME_INFORMATION`.
    VolumeInformation,
    /// `$DATA`.
    Data,
    /// `$INDEX_ROOT`.
    IndexRoot,
    /// `$INDEX_ALLOCATION`.
    IndexAllocation,
    /// `$BITMAP`.
    Bitmap,
    /// `$REPARSE_POINT`.
    ReparsePoint,
    /// `$EA_INFORMATION`.
    EaInformation,
    /// `$EA`.
    Ea,
    /// `$LOGGED_UTILITY_STREAM`.
    LoggedUtilityStream,
    /// Anything else.
    Unknown(u32),
}

impl AttributeType {
    /// Decodes a type code.
    #[must_use]
    pub const fn from_code(code: u32) -> Self {
        match code {
            0x10 => Self::StandardInformation,
            0x20 => Self::AttributeList,
            0x30 => Self::FileName,
            0x40 => Self::ObjectId,
            0x50 => Self::SecurityDescriptor,
            0x60 => Self::VolumeName,
            0x70 => Self::VolumeInformation,
            0x80 => Self::Data,
            0x90 => Self::IndexRoot,
            0xA0 => Self::IndexAllocation,
            0xB0 => Self::Bitmap,
            0xC0 => Self::ReparsePoint,
            0xD0 => Self::EaInformation,
            0xE0 => Self::Ea,
            0x100 => Self::LoggedUtilityStream,
            other => Self::Unknown(other),
        }
    }

    /// The numeric type code.
    #[must_use]
    pub const fn code(&self) -> u32 {
        match self {
            Self::StandardInformation => 0x10,
            Self::AttributeList => 0x20,
            Self::FileName => 0x30,
            Self::ObjectId => 0x40,
            Self::SecurityDescriptor => 0x50,
            Self::VolumeName => 0x60,
            Self::VolumeInformation => 0x70,
            Self::Data => 0x80,
            Self::IndexRoot => 0x90,
            Self::IndexAllocation => 0xA0,
            Self::Bitmap => 0xB0,
            Self::ReparsePoint => 0xC0,
            Self::EaInformation => 0xD0,
            Self::Ea => 0xE0,
            Self::LoggedUtilityStream => 0x100,
            Self::Unknown(c) => *c,
        }
    }

    /// Canonical name (`$DATA`, …).
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::StandardInformation => "$STANDARD_INFORMATION",
            Self::AttributeList => "$ATTRIBUTE_LIST",
            Self::FileName => "$FILE_NAME",
            Self::ObjectId => "$OBJECT_ID",
            Self::SecurityDescriptor => "$SECURITY_DESCRIPTOR",
            Self::VolumeName => "$VOLUME_NAME",
            Self::VolumeInformation => "$VOLUME_INFORMATION",
            Self::Data => "$DATA",
            Self::IndexRoot => "$INDEX_ROOT",
            Self::IndexAllocation => "$INDEX_ALLOCATION",
            Self::Bitmap => "$BITMAP",
            Self::ReparsePoint => "$REPARSE_POINT",
            Self::EaInformation => "$EA_INFORMATION",
            Self::Ea => "$EA",
            Self::LoggedUtilityStream => "$LOGGED_UTILITY_STREAM",
            Self::Unknown(_) => "$UNKNOWN",
        }
    }
}

impl std::fmt::Display for AttributeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown(c) => write!(f, "$UNKNOWN({c:#x})"),
            other => f.write_str(other.name()),
        }
    }
}

/// Common attribute header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeHeader {
    /// Type.
    pub attribute_type: AttributeType,
    /// Total length of the attribute inside the record.
    pub length: u32,
    /// Whether the value is stored outside the record.
    pub non_resident: bool,
    /// Attribute name (empty for the unnamed `$DATA` stream), or `None` when
    /// no name is present.
    pub name: Option<String>,
    /// Raw flags.
    pub flags: u16,
    /// Attribute identifier within the record.
    pub id: u16,
}

impl AttributeHeader {
    /// Whether the compressed flag is set.
    #[must_use]
    pub const fn is_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSION_MASK != 0
    }

    /// Whether the encrypted flag is set.
    #[must_use]
    pub const fn is_encrypted(&self) -> bool {
        self.flags & FLAG_ENCRYPTED != 0
    }

    /// Whether the sparse flag is set.
    #[must_use]
    pub const fn is_sparse(&self) -> bool {
        self.flags & FLAG_SPARSE != 0
    }
}

/// Non-resident attribute header fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NonResidentHeader {
    /// First VCN covered by this attribute (non-zero in extension records).
    pub starting_vcn: u64,
    /// Last VCN covered.
    pub last_vcn: u64,
    /// Offset of the mapping pairs inside the attribute.
    pub runlist_offset: u16,
    /// Compression unit exponent (0 when not compressed).
    pub compression_unit: u8,
    /// Allocated size in bytes (only valid when `starting_vcn == 0`).
    pub allocated_size: u64,
    /// Real (logical) size in bytes.
    pub real_size: u64,
    /// Initialised size in bytes; data beyond it reads as zero.
    pub initialized_size: u64,
}

/// Attribute body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributeBody<'a> {
    /// Value stored inside the record.
    Resident {
        /// The value bytes.
        value: &'a [u8],
        /// Resident flags (bit 0: indexed).
        flags: u8,
    },
    /// Value stored in clusters described by a runlist.
    NonResident {
        /// Header fields.
        header: NonResidentHeader,
        /// Raw mapping pairs (up to the end of the attribute).
        mapping_pairs: &'a [u8],
    },
}

/// One attribute of a FILE record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute<'a> {
    /// Byte offset of the attribute inside the record.
    pub offset: usize,
    /// Header.
    pub header: AttributeHeader,
    /// Body.
    pub body: AttributeBody<'a>,
}

impl<'a> Attribute<'a> {
    /// The resident value, if the attribute is resident.
    #[must_use]
    pub const fn resident_value(&self) -> Option<&'a [u8]> {
        match &self.body {
            AttributeBody::Resident { value, .. } => Some(value),
            AttributeBody::NonResident { .. } => None,
        }
    }

    /// The non-resident header, if the attribute is non-resident.
    #[must_use]
    pub const fn non_resident(&self) -> Option<&NonResidentHeader> {
        match &self.body {
            AttributeBody::NonResident { header, .. } => Some(header),
            AttributeBody::Resident { .. } => None,
        }
    }

    /// Whether this is the unnamed (default) stream of its type.
    #[must_use]
    pub fn is_unnamed(&self) -> bool {
        self.header.name.as_deref().is_none_or(str::is_empty)
    }

    /// Parses one attribute at `offset` of `record`.
    ///
    /// Returns `Ok(None)` at the end marker.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::InvalidAttribute`] for a malformed header.
    pub fn parse(
        record_number: u64,
        record: &'a [u8],
        offset: usize,
        limit: usize,
    ) -> Result<Option<Self>, NtfsError> {
        let invalid = |reason: &str| NtfsError::InvalidAttribute {
            record: record_number,
            offset,
            reason: reason.to_owned(),
        };
        let view = ByteView::new(record);
        let type_code = view
            .u32_le(offset)
            .ok_or_else(|| invalid("truncated before type"))?;
        if type_code == END_MARKER {
            return Ok(None);
        }
        let length = view
            .u32_le(offset + 4)
            .ok_or_else(|| invalid("truncated before length"))?;
        let length_usize = usize::try_from(length).map_err(|_| invalid("length overflows"))?;
        if length < 24 || length % 8 != 0 {
            return Err(invalid(&format!("length {length} is invalid")));
        }
        let end = offset
            .checked_add(length_usize)
            .ok_or_else(|| invalid("length overflows"))?;
        if end > limit || end > record.len() {
            return Err(invalid("attribute extends beyond the record"));
        }
        let attr = view
            .sub(offset, length_usize)
            .ok_or_else(|| invalid("attribute extends beyond the record"))?;
        let non_resident = attr.u8(8).ok_or_else(|| invalid("truncated"))? != 0;
        let name_length = usize::from(attr.u8(9).ok_or_else(|| invalid("truncated"))?);
        let name_offset = usize::from(attr.u16_le(10).ok_or_else(|| invalid("truncated"))?);
        let flags = attr.u16_le(12).ok_or_else(|| invalid("truncated"))?;
        let id = attr.u16_le(14).ok_or_else(|| invalid("truncated"))?;
        let name = if name_length == 0 {
            None
        } else {
            let bytes = attr
                .slice(name_offset, name_length * 2)
                .ok_or_else(|| invalid("name extends beyond the attribute"))?;
            Some(utf16le_to_string_lossy(bytes))
        };
        let header = AttributeHeader {
            attribute_type: AttributeType::from_code(type_code),
            length,
            non_resident,
            name,
            flags,
            id,
        };

        let body = if non_resident {
            if length < 64 {
                return Err(invalid("non-resident attribute shorter than its header"));
            }
            let nr = NonResidentHeader {
                starting_vcn: attr.u64_le(16).ok_or_else(|| invalid("truncated"))?,
                last_vcn: attr.u64_le(24).ok_or_else(|| invalid("truncated"))?,
                runlist_offset: attr.u16_le(32).ok_or_else(|| invalid("truncated"))?,
                compression_unit: attr.u8(34).ok_or_else(|| invalid("truncated"))?,
                allocated_size: attr.u64_le(40).ok_or_else(|| invalid("truncated"))?,
                real_size: attr.u64_le(48).ok_or_else(|| invalid("truncated"))?,
                initialized_size: attr.u64_le(56).ok_or_else(|| invalid("truncated"))?,
            };
            let ro = usize::from(nr.runlist_offset);
            if ro < 64 || ro > length_usize {
                return Err(invalid("runlist offset is outside the attribute"));
            }
            if nr.last_vcn < nr.starting_vcn
                && !(nr.last_vcn == 0 && nr.starting_vcn == 0)
                && nr.allocated_size != 0
            {
                return Err(invalid("last VCN precedes starting VCN"));
            }
            let mapping_pairs = attr
                .slice(ro, length_usize - ro)
                .ok_or_else(|| invalid("runlist truncated"))?;
            AttributeBody::NonResident {
                header: nr,
                mapping_pairs,
            }
        } else {
            let value_length =
                usize::try_from(attr.u32_le(16).ok_or_else(|| invalid("truncated"))?)
                    .map_err(|_| invalid("value length overflows"))?;
            let value_offset = usize::from(attr.u16_le(20).ok_or_else(|| invalid("truncated"))?);
            let resident_flags = attr.u8(22).ok_or_else(|| invalid("truncated"))?;
            let value_end = value_offset
                .checked_add(value_length)
                .ok_or_else(|| invalid("value overflows"))?;
            if value_offset < 24 || value_end > length_usize {
                return Err(invalid("resident value lies outside the attribute"));
            }
            let value = attr
                .slice(value_offset, value_length)
                .ok_or_else(|| invalid("resident value truncated"))?;
            AttributeBody::Resident {
                value,
                flags: resident_flags,
            }
        };
        Ok(Some(Self {
            offset,
            header,
            body,
        }))
    }
}

/// Iterator over the attributes of a record.
///
/// A malformed attribute yields one error and then terminates the iteration
/// so that the caller can keep whatever it parsed before the damage.
#[derive(Debug, Clone)]
pub struct AttributeIter<'a> {
    record_number: u64,
    record: &'a [u8],
    pos: usize,
    limit: usize,
    done: bool,
}

impl<'a> AttributeIter<'a> {
    /// Creates an iterator starting at `first_offset` and stopping at `limit`.
    #[must_use]
    pub const fn new(
        record_number: u64,
        record: &'a [u8],
        first_offset: usize,
        limit: usize,
    ) -> Self {
        Self {
            record_number,
            record,
            pos: first_offset,
            limit,
            done: false,
        }
    }
}

impl<'a> Iterator for AttributeIter<'a> {
    type Item = Result<Attribute<'a>, NtfsError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // The end marker needs four bytes; allow it to sit at the limit.
        if self.pos.checked_add(4)? > self.record.len().min(self.limit.saturating_add(4)) {
            self.done = true;
            return Some(Err(NtfsError::InvalidAttribute {
                record: self.record_number,
                offset: self.pos,
                reason: "end marker missing".into(),
            }));
        }
        match Attribute::parse(self.record_number, self.record, self.pos, self.limit) {
            Ok(Some(attr)) => {
                self.pos = self
                    .pos
                    .saturating_add(usize::try_from(attr.header.length).unwrap_or(usize::MAX));
                Some(Ok(attr))
            }
            Ok(None) => {
                self.done = true;
                None
            }
            Err(e) => {
                self.done = true;
                Some(Err(e))
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builders for synthetic attributes.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    /// Builds a resident attribute.
    pub fn resident(type_code: u32, name: Option<&str>, value: &[u8], flags: u16) -> Vec<u8> {
        let name_utf16: Vec<u8> = name
            .map(|n| n.encode_utf16().flat_map(u16::to_le_bytes).collect())
            .unwrap_or_default();
        let name_offset = 24usize;
        let value_offset = (name_offset + name_utf16.len()).div_ceil(8) * 8;
        let total = (value_offset + value.len()).div_ceil(8) * 8;
        let mut a = vec![0u8; total];
        a[..4].copy_from_slice(&type_code.to_le_bytes());
        a[4..8].copy_from_slice(&(total as u32).to_le_bytes());
        a[8] = 0;
        a[9] = (name_utf16.len() / 2) as u8;
        a[10..12].copy_from_slice(&(name_offset as u16).to_le_bytes());
        a[12..14].copy_from_slice(&flags.to_le_bytes());
        a[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
        a[20..22].copy_from_slice(&(value_offset as u16).to_le_bytes());
        a[name_offset..name_offset + name_utf16.len()].copy_from_slice(&name_utf16);
        a[value_offset..value_offset + value.len()].copy_from_slice(value);
        a
    }

    /// Builds a non-resident attribute with the given mapping pairs.
    pub fn non_resident(
        type_code: u32,
        name: Option<&str>,
        mapping_pairs: &[u8],
        starting_vcn: u64,
        last_vcn: u64,
        sizes: (u64, u64, u64),
        flags: u16,
    ) -> Vec<u8> {
        let name_utf16: Vec<u8> = name
            .map(|n| n.encode_utf16().flat_map(u16::to_le_bytes).collect())
            .unwrap_or_default();
        let name_offset = 64usize;
        let run_offset = (name_offset + name_utf16.len()).div_ceil(8) * 8;
        let total = (run_offset + mapping_pairs.len() + 1).div_ceil(8) * 8;
        let mut a = vec![0u8; total];
        a[..4].copy_from_slice(&type_code.to_le_bytes());
        a[4..8].copy_from_slice(&(total as u32).to_le_bytes());
        a[8] = 1;
        a[9] = (name_utf16.len() / 2) as u8;
        a[10..12].copy_from_slice(&(name_offset as u16).to_le_bytes());
        a[12..14].copy_from_slice(&flags.to_le_bytes());
        a[16..24].copy_from_slice(&starting_vcn.to_le_bytes());
        a[24..32].copy_from_slice(&last_vcn.to_le_bytes());
        a[32..34].copy_from_slice(&(run_offset as u16).to_le_bytes());
        a[40..48].copy_from_slice(&sizes.0.to_le_bytes());
        a[48..56].copy_from_slice(&sizes.1.to_le_bytes());
        a[56..64].copy_from_slice(&sizes.2.to_le_bytes());
        a[name_offset..name_offset + name_utf16.len()].copy_from_slice(&name_utf16);
        a[run_offset..run_offset + mapping_pairs.len()].copy_from_slice(mapping_pairs);
        a
    }

    /// Concatenates attributes and appends the end marker.
    pub fn attributes(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        for p in parts {
            out.extend_from_slice(p);
        }
        out.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
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

    use super::testutil::{attributes, non_resident, resident};
    use super::*;

    fn iter(record: &[u8]) -> Vec<Result<Attribute<'_>, NtfsError>> {
        AttributeIter::new(1, record, 0, record.len()).collect()
    }

    #[test]
    fn resident_and_non_resident() {
        let rec = attributes(&[
            resident(0x10, None, &[1, 2, 3, 4], 0),
            resident(0x80, Some("stream"), b"hello", 0),
            non_resident(
                0x80,
                None,
                &[0x11, 0x08, 0x20, 0x00],
                0,
                7,
                (32768, 30000, 30000),
                0,
            ),
        ]);
        let attrs: Vec<_> = iter(&rec).into_iter().map(Result::unwrap).collect();
        assert_eq!(attrs.len(), 3);
        assert_eq!(
            attrs[0].header.attribute_type,
            AttributeType::StandardInformation
        );
        assert_eq!(attrs[0].resident_value(), Some(&[1, 2, 3, 4][..]));
        assert!(attrs[0].is_unnamed());
        assert_eq!(attrs[1].header.name.as_deref(), Some("stream"));
        assert_eq!(attrs[1].resident_value(), Some(&b"hello"[..]));
        assert!(!attrs[1].is_unnamed());
        let nr = attrs[2].non_resident().unwrap();
        assert_eq!(nr.last_vcn, 7);
        assert_eq!(nr.real_size, 30000);
        assert_eq!(
            attrs[2].offset,
            attrs[0].header.length as usize + attrs[1].header.length as usize
        );
        match &attrs[2].body {
            AttributeBody::NonResident { mapping_pairs, .. } => {
                assert_eq!(&mapping_pairs[..4], &[0x11, 0x08, 0x20, 0x00])
            }
            AttributeBody::Resident { .. } => panic!(),
        }
    }

    #[test]
    fn unknown_attribute_is_skipped_not_fatal() {
        let rec = attributes(&[
            resident(0x1234, None, &[9], 0),
            resident(0x30, None, &[1], 0),
        ]);
        let attrs: Vec<_> = iter(&rec).into_iter().map(Result::unwrap).collect();
        assert_eq!(
            attrs[0].header.attribute_type,
            AttributeType::Unknown(0x1234)
        );
        assert_eq!(attrs[1].header.attribute_type, AttributeType::FileName);
    }

    #[test]
    fn malformed_lengths_yield_one_error() {
        // Zero-length attribute.
        let mut rec = attributes(&[resident(0x10, None, &[1], 0)]);
        rec[4..8].copy_from_slice(&0u32.to_le_bytes());
        let items = iter(&rec);
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Err(NtfsError::InvalidAttribute { .. })));

        // Attribute beyond record.
        let mut rec = attributes(&[resident(0x10, None, &[1], 0)]);
        rec[4..8].copy_from_slice(&4096u32.to_le_bytes());
        assert!(matches!(
            iter(&rec)[0],
            Err(NtfsError::InvalidAttribute { .. })
        ));

        // Resident value outside attribute.
        let mut rec = attributes(&[resident(0x10, None, &[1], 0)]);
        rec[16..20].copy_from_slice(&500u32.to_le_bytes());
        assert!(matches!(
            iter(&rec)[0],
            Err(NtfsError::InvalidAttribute { .. })
        ));

        // Missing end marker: record ends right after a valid attribute.
        let rec = resident(0x10, None, &[1], 0);
        let items = iter(&rec);
        assert_eq!(items.len(), 2);
        assert!(items[0].is_ok());
        assert!(items[1].is_err());
    }

    #[test]
    fn flags_and_type_names() {
        let h = AttributeHeader {
            attribute_type: AttributeType::Data,
            length: 24,
            non_resident: true,
            name: None,
            flags: FLAG_COMPRESSED | FLAG_SPARSE,
            id: 0,
        };
        assert!(h.is_compressed());
        assert!(h.is_sparse());
        assert!(!h.is_encrypted());
        assert_eq!(AttributeType::from_code(0x80).to_string(), "$DATA");
        assert_eq!(
            AttributeType::from_code(0x999).to_string(),
            "$UNKNOWN(0x999)"
        );
        assert_eq!(AttributeType::Data.code(), 0x80);
    }
}
