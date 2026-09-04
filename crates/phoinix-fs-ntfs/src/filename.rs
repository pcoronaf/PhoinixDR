//! `$FILE_NAME` attribute.

use phoinix_core::bytes::{ByteView, utf16le_to_string, utf16le_to_string_lossy};
use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::record::FileReference;
use crate::timestamp::NtfsTimestamp;

/// Namespace of a filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileNameNamespace {
    /// POSIX: case sensitive, any character except `/` and NUL.
    Posix,
    /// Win32 long name.
    Win32,
    /// DOS 8.3 short name.
    Dos,
    /// A name that is both Win32 and DOS compatible.
    Win32AndDos,
    /// Unrecognised namespace byte.
    Unknown(u8),
}

impl FileNameNamespace {
    /// Decodes the namespace byte.
    #[must_use]
    pub const fn from_byte(b: u8) -> Self {
        match b {
            0 => Self::Posix,
            1 => Self::Win32,
            2 => Self::Dos,
            3 => Self::Win32AndDos,
            other => Self::Unknown(other),
        }
    }

    /// Ranking for display preference: Win32 names first, DOS names last.
    #[must_use]
    pub const fn display_rank(&self) -> u8 {
        match self {
            Self::Win32AndDos => 0,
            Self::Win32 => 1,
            Self::Posix => 2,
            Self::Unknown(_) => 3,
            Self::Dos => 4,
        }
    }
}

/// File attribute flag: directory (in `$FILE_NAME.flags`, bit 28).
pub const FILE_ATTR_DIRECTORY_FN: u32 = 0x1000_0000;
/// File attribute flag: compressed.
pub const FILE_ATTR_COMPRESSED: u32 = 0x0800;
/// File attribute flag: encrypted.
pub const FILE_ATTR_ENCRYPTED: u32 = 0x4000;
/// File attribute flag: sparse.
pub const FILE_ATTR_SPARSE: u32 = 0x0200;
/// File attribute flag: reparse point.
pub const FILE_ATTR_REPARSE_POINT: u32 = 0x0400;

/// Parsed `$FILE_NAME` attribute value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileNameAttribute {
    /// Parent directory reference (with sequence number).
    pub parent: FileReference,
    /// Name.
    pub name: String,
    /// Namespace.
    pub namespace: FileNameNamespace,
    /// Creation time.
    pub created: NtfsTimestamp,
    /// Last data modification time.
    pub modified: NtfsTimestamp,
    /// Last MFT record modification time.
    pub mft_modified: NtfsTimestamp,
    /// Last access time.
    pub accessed: NtfsTimestamp,
    /// Allocated size of the unnamed data stream (may be stale).
    pub allocated_size: u64,
    /// Real size of the unnamed data stream (may be stale).
    pub real_size: u64,
    /// File attribute flags.
    pub flags: u32,
    /// Reparse tag or extended-attribute size.
    pub reparse_value: u32,
    /// Whether the name failed strict UTF-16 decoding and was decoded lossily.
    pub name_invalid_utf16: bool,
}

impl FileNameAttribute {
    /// Parses a resident `$FILE_NAME` value.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::InvalidAttribute`] if the value is truncated.
    pub fn parse(record: u64, offset: usize, value: &[u8]) -> Result<Self, NtfsError> {
        let invalid = |reason: &str| NtfsError::InvalidAttribute {
            record,
            offset,
            reason: reason.to_owned(),
        };
        let view = ByteView::new(value);
        let parent = FileReference::from_raw(
            view.u64_le(0)
                .ok_or_else(|| invalid("$FILE_NAME truncated"))?,
        );
        let created = NtfsTimestamp::new(
            view.u64_le(8)
                .ok_or_else(|| invalid("$FILE_NAME truncated"))?,
        );
        let modified = NtfsTimestamp::new(
            view.u64_le(16)
                .ok_or_else(|| invalid("$FILE_NAME truncated"))?,
        );
        let mft_modified = NtfsTimestamp::new(
            view.u64_le(24)
                .ok_or_else(|| invalid("$FILE_NAME truncated"))?,
        );
        let accessed = NtfsTimestamp::new(
            view.u64_le(32)
                .ok_or_else(|| invalid("$FILE_NAME truncated"))?,
        );
        let allocated_size = view
            .u64_le(40)
            .ok_or_else(|| invalid("$FILE_NAME truncated"))?;
        let real_size = view
            .u64_le(48)
            .ok_or_else(|| invalid("$FILE_NAME truncated"))?;
        let flags = view
            .u32_le(56)
            .ok_or_else(|| invalid("$FILE_NAME truncated"))?;
        let reparse_value = view
            .u32_le(60)
            .ok_or_else(|| invalid("$FILE_NAME truncated"))?;
        let name_length = usize::from(view.u8(64).ok_or_else(|| invalid("$FILE_NAME truncated"))?);
        let namespace = FileNameNamespace::from_byte(
            view.u8(65).ok_or_else(|| invalid("$FILE_NAME truncated"))?,
        );
        let name_bytes = view
            .slice(66, name_length * 2)
            .ok_or_else(|| invalid("$FILE_NAME name length exceeds the attribute"))?;
        let (name, name_invalid_utf16) = match utf16le_to_string(name_bytes) {
            Some(n) => (n, false),
            None => (utf16le_to_string_lossy(name_bytes), true),
        };
        Ok(Self {
            parent,
            name,
            namespace,
            created,
            modified,
            mft_modified,
            accessed,
            allocated_size,
            real_size,
            flags,
            reparse_value,
            name_invalid_utf16,
        })
    }

    /// Whether the directory flag is set.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        self.flags & FILE_ATTR_DIRECTORY_FN != 0
    }
}

/// Picks the best name for display from a set of `$FILE_NAME` attributes.
#[must_use]
pub fn preferred_name(names: &[FileNameAttribute]) -> Option<&FileNameAttribute> {
    names.iter().min_by_key(|n| n.namespace.display_rank())
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builder for `$FILE_NAME` values.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    use crate::record::FileReference;

    pub fn file_name_value(
        parent: FileReference,
        name: &str,
        namespace: u8,
        real_size: u64,
        flags: u32,
    ) -> Vec<u8> {
        let utf16: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let mut v = vec![0u8; 66 + utf16.len()];
        v[..8].copy_from_slice(&parent.to_raw().to_le_bytes());
        v[8..16].copy_from_slice(&0x01DC_0000_0000_0000u64.to_le_bytes());
        v[16..24].copy_from_slice(&0x01DC_0000_0000_0001u64.to_le_bytes());
        v[24..32].copy_from_slice(&0x01DC_0000_0000_0002u64.to_le_bytes());
        v[32..40].copy_from_slice(&0x01DC_0000_0000_0003u64.to_le_bytes());
        v[40..48].copy_from_slice(&real_size.div_ceil(4096).saturating_mul(4096).to_le_bytes());
        v[48..56].copy_from_slice(&real_size.to_le_bytes());
        v[56..60].copy_from_slice(&flags.to_le_bytes());
        v[64] = (utf16.len() / 2) as u8;
        v[65] = namespace;
        v[66..].copy_from_slice(&utf16);
        v
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

    use super::testutil::file_name_value;
    use super::*;

    #[test]
    fn parses_win32_dos_and_unicode_names() {
        let parent = FileReference {
            record: 5,
            sequence: 5,
        };
        let v = file_name_value(parent, "Ünïcödé 文件.txt", 1, 1234, 0x20);
        let fnm = FileNameAttribute::parse(7, 0, &v).unwrap();
        assert_eq!(fnm.name, "Ünïcödé 文件.txt");
        assert_eq!(fnm.namespace, FileNameNamespace::Win32);
        assert_eq!(fnm.parent, parent);
        assert_eq!(fnm.real_size, 1234);
        assert!(!fnm.is_directory());
        assert!(!fnm.name_invalid_utf16);

        let dos =
            FileNameAttribute::parse(7, 0, &file_name_value(parent, "UNICOD~1.TXT", 2, 1234, 0))
                .unwrap();
        assert_eq!(dos.namespace, FileNameNamespace::Dos);
        let names = vec![dos, fnm.clone()];
        assert_eq!(preferred_name(&names).unwrap().name, fnm.name);
    }

    #[test]
    fn invalid_utf16_is_lossy_and_flagged() {
        let mut v = file_name_value(
            FileReference {
                record: 5,
                sequence: 5,
            },
            "ab",
            1,
            0,
            0,
        );
        v[66..68].copy_from_slice(&[0x00, 0xD8]);
        let fnm = FileNameAttribute::parse(7, 0, &v).unwrap();
        assert!(fnm.name_invalid_utf16);
        assert!(fnm.name.contains('\u{FFFD}'));
    }

    #[test]
    fn truncated_value_is_rejected() {
        let v = file_name_value(
            FileReference {
                record: 5,
                sequence: 5,
            },
            "abc",
            1,
            0,
            0,
        );
        assert!(FileNameAttribute::parse(7, 0, &v[..60]).is_err());
        let mut short = v.clone();
        short[64] = 50; // claims 50 characters
        assert!(FileNameAttribute::parse(7, 0, &short).is_err());
    }
}
