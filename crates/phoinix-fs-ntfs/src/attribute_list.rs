//! `$ATTRIBUTE_LIST` parsing.

use phoinix_core::bytes::{ByteView, utf16le_to_string_lossy};
use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::attribute::AttributeType;
use crate::record::FileReference;

/// One entry of an attribute list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeListEntry {
    /// Attribute type.
    pub attribute_type: AttributeType,
    /// Attribute name (empty for unnamed).
    pub name: String,
    /// First VCN held by this piece (non-resident attributes split across
    /// records).
    pub starting_vcn: u64,
    /// Record holding the attribute.
    pub reference: FileReference,
    /// Attribute identifier inside that record.
    pub attribute_id: u16,
}

/// Parses the bytes of an attribute list.
///
/// # Errors
///
/// Returns [`NtfsError::InvalidAttribute`] for malformed entries.
pub fn parse_attribute_list(
    record: u64,
    bytes: &[u8],
) -> Result<Vec<AttributeListEntry>, NtfsError> {
    let view = ByteView::new(bytes);
    let mut entries = Vec::new();
    let mut pos = 0usize;
    while pos < bytes.len() {
        let invalid = |reason: &str| NtfsError::InvalidAttribute {
            record,
            offset: pos,
            reason: format!("$ATTRIBUTE_LIST: {reason}"),
        };
        // Allow trailing zero padding.
        if bytes.len() - pos < 26 {
            if bytes
                .get(pos..)
                .is_some_and(|rest| rest.iter().all(|b| *b == 0))
            {
                break;
            }
            return Err(invalid("truncated entry"));
        }
        let type_code = view.u32_le(pos).ok_or_else(|| invalid("truncated"))?;
        let length = usize::from(view.u16_le(pos + 4).ok_or_else(|| invalid("truncated"))?);
        if type_code == 0 && length == 0 {
            break;
        }
        if length < 26 || length % 8 != 0 {
            return Err(invalid(&format!("entry length {length} invalid")));
        }
        let end = pos.checked_add(length).ok_or_else(|| invalid("overflow"))?;
        if end > bytes.len() {
            return Err(invalid("entry extends beyond the list"));
        }
        let name_length = usize::from(view.u8(pos + 6).ok_or_else(|| invalid("truncated"))?);
        let name_offset = usize::from(view.u8(pos + 7).ok_or_else(|| invalid("truncated"))?);
        let starting_vcn = view.u64_le(pos + 8).ok_or_else(|| invalid("truncated"))?;
        let reference =
            FileReference::from_raw(view.u64_le(pos + 16).ok_or_else(|| invalid("truncated"))?);
        let attribute_id = view.u16_le(pos + 24).ok_or_else(|| invalid("truncated"))?;
        let name = if name_length == 0 {
            String::new()
        } else {
            let start = pos
                .checked_add(name_offset)
                .ok_or_else(|| invalid("overflow"))?;
            let name_bytes = view
                .slice(start, name_length * 2)
                .ok_or_else(|| invalid("name extends beyond entry"))?;
            if start + name_length * 2 > end {
                return Err(invalid("name extends beyond entry"));
            }
            utf16le_to_string_lossy(name_bytes)
        };
        entries.push(AttributeListEntry {
            attribute_type: AttributeType::from_code(type_code),
            name,
            starting_vcn,
            reference,
            attribute_id,
        });
        pos = end;
    }
    Ok(entries)
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builder for attribute-list bytes.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    use crate::record::FileReference;

    pub fn entry(
        type_code: u32,
        name: &str,
        starting_vcn: u64,
        reference: FileReference,
        id: u16,
    ) -> Vec<u8> {
        let utf16: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let len = (26 + utf16.len()).div_ceil(8) * 8;
        let mut e = vec![0u8; len];
        e[..4].copy_from_slice(&type_code.to_le_bytes());
        e[4..6].copy_from_slice(&(len as u16).to_le_bytes());
        e[6] = (utf16.len() / 2) as u8;
        e[7] = 26;
        e[8..16].copy_from_slice(&starting_vcn.to_le_bytes());
        e[16..24].copy_from_slice(&reference.to_raw().to_le_bytes());
        e[24..26].copy_from_slice(&id.to_le_bytes());
        e[26..26 + utf16.len()].copy_from_slice(&utf16);
        e
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

    use super::testutil::entry;
    use super::*;

    #[test]
    fn parses_entries() {
        let r0 = FileReference {
            record: 0,
            sequence: 1,
        };
        let r15 = FileReference {
            record: 15,
            sequence: 15,
        };
        let mut list = entry(0x10, "", 0, r0, 0);
        list.extend(entry(0x80, "", 0, r0, 3));
        list.extend(entry(0x80, "", 4096, r15, 1));
        list.extend(entry(0x80, "alt", 0, r0, 5));
        list.extend([0u8; 8]);
        let entries = parse_attribute_list(0, &list).unwrap();
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[2].starting_vcn, 4096);
        assert_eq!(entries[2].reference, r15);
        assert_eq!(entries[3].name, "alt");
        assert_eq!(entries[3].attribute_type, AttributeType::Data);
    }

    #[test]
    fn rejects_malformed() {
        let mut list = entry(
            0x80,
            "",
            0,
            FileReference {
                record: 0,
                sequence: 1,
            },
            0,
        );
        list[4..6].copy_from_slice(&8u16.to_le_bytes());
        assert!(parse_attribute_list(0, &list).is_err());
        let list = entry(
            0x80,
            "abc",
            0,
            FileReference {
                record: 0,
                sequence: 1,
            },
            0,
        );
        assert!(parse_attribute_list(0, &list[..28]).is_err());
    }
}
