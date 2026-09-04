//! Filesystem type enumeration.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Filesystems PHOINIX knows how to name. Only some are implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum FileSystemType {
    /// Microsoft NTFS.
    Ntfs,
    /// FAT12.
    Fat12,
    /// FAT16.
    Fat16,
    /// FAT32.
    Fat32,
    /// exFAT.
    ExFat,
    /// ext2/ext3/ext4 family.
    Ext,
    /// Classic HFS.
    Hfs,
    /// HFS+ / HFSX.
    HfsPlus,
    /// APFS container or volume.
    Apfs,
    /// Not recognised.
    Unknown,
}

impl FileSystemType {
    /// Short, user-facing label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            FileSystemType::Ntfs => "NTFS",
            FileSystemType::Fat12 => "FAT12",
            FileSystemType::Fat16 => "FAT16",
            FileSystemType::Fat32 => "FAT32",
            FileSystemType::ExFat => "exFAT",
            FileSystemType::Ext => "EXT",
            FileSystemType::Hfs => "HFS",
            FileSystemType::HfsPlus => "HFS+",
            FileSystemType::Apfs => "APFS",
            FileSystemType::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for FileSystemType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}
