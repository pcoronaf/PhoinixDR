//! `$STANDARD_INFORMATION` attribute.

use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::timestamp::NtfsTimestamp;

/// Parsed `$STANDARD_INFORMATION` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StandardInformation {
    /// Creation time.
    pub created: NtfsTimestamp,
    /// Last data modification time.
    pub modified: NtfsTimestamp,
    /// Last MFT record modification time.
    pub mft_modified: NtfsTimestamp,
    /// Last access time.
    pub accessed: NtfsTimestamp,
    /// File attribute flags (`FILE_ATTRIBUTE_*`).
    pub file_attributes: u32,
    /// Owner identifier (NTFS 3.0+), if present.
    pub owner_id: Option<u32>,
    /// Security identifier (NTFS 3.0+), if present.
    pub security_id: Option<u32>,
    /// Quota charged (NTFS 3.0+), if present.
    pub quota_charged: Option<u64>,
    /// Update sequence number (NTFS 3.0+), if present.
    pub usn: Option<u64>,
}

impl StandardInformation {
    /// Parses a resident value of 48 (NTFS 1.x) or 72 (NTFS 3.x) bytes.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::InvalidAttribute`] if the value is shorter than
    /// the 36 mandatory bytes.
    pub fn parse(record: u64, offset: usize, value: &[u8]) -> Result<Self, NtfsError> {
        let invalid = || NtfsError::InvalidAttribute {
            record,
            offset,
            reason: "$STANDARD_INFORMATION truncated".into(),
        };
        let view = ByteView::new(value);
        Ok(Self {
            created: NtfsTimestamp::new(view.u64_le(0).ok_or_else(invalid)?),
            modified: NtfsTimestamp::new(view.u64_le(8).ok_or_else(invalid)?),
            mft_modified: NtfsTimestamp::new(view.u64_le(16).ok_or_else(invalid)?),
            accessed: NtfsTimestamp::new(view.u64_le(24).ok_or_else(invalid)?),
            file_attributes: view.u32_le(32).ok_or_else(invalid)?,
            owner_id: view.u32_le(48),
            security_id: view.u32_le(52),
            quota_charged: view.u64_le(56),
            usn: view.u64_le(64),
        })
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
    fn parses_short_and_long_forms() {
        let mut v = vec![0u8; 72];
        v[..8].copy_from_slice(&1u64.to_le_bytes());
        v[32..36].copy_from_slice(&0x20u32.to_le_bytes());
        v[52..56].copy_from_slice(&0x104u32.to_le_bytes());
        let si = StandardInformation::parse(1, 0, &v).unwrap();
        assert_eq!(si.created.raw, 1);
        assert_eq!(si.file_attributes, 0x20);
        assert_eq!(si.security_id, Some(0x104));
        let short = StandardInformation::parse(1, 0, &v[..48]).unwrap();
        assert_eq!(short.security_id, None);
        assert!(StandardInformation::parse(1, 0, &v[..30]).is_err());
    }
}
