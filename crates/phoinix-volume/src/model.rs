//! Generic partition-table model shared by MBR and GPT.

use std::fmt;
use std::sync::Arc;

use bitflags::bitflags;
use phoinix_block::{BlockReader, SubrangeReader};
use phoinix_core::{ByteRange, arith};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{VolumeDiagnostic, VolumeError};

/// Partitioning scheme found on a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PartitionScheme {
    /// Classic MBR (possibly with extended partitions).
    Mbr,
    /// GUID Partition Table.
    Gpt,
    /// No partition table: the source is a bare volume or empty.
    None,
    /// Something that could not be interpreted.
    Unknown,
}

impl fmt::Display for PartitionScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PartitionScheme::Mbr => "MBR",
            PartitionScheme::Gpt => "GPT",
            PartitionScheme::None => "none",
            PartitionScheme::Unknown => "unknown",
        })
    }
}

/// Partition type as recorded in the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scheme", content = "value", rename_all = "lowercase")]
pub enum PartitionType {
    /// MBR type byte.
    Mbr(u8),
    /// GPT type GUID.
    Gpt(Uuid),
}

/// Well-known GPT type GUIDs.
pub mod gpt_types {
    use uuid::{Uuid, uuid};

    /// EFI System Partition.
    pub const EFI_SYSTEM: Uuid = uuid!("C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
    /// Microsoft Reserved.
    pub const MICROSOFT_RESERVED: Uuid = uuid!("E3C9E316-0B5C-4DB8-817D-F92DF00215AE");
    /// Microsoft Basic Data.
    pub const BASIC_DATA: Uuid = uuid!("EBD0A0A2-B9E5-4433-87C0-68B6B72699C7");
    /// Windows Recovery Environment.
    pub const WINDOWS_RECOVERY: Uuid = uuid!("DE94BBA4-06D1-4D40-A16A-BFD50179D6AC");
    /// Linux filesystem data.
    pub const LINUX_FILESYSTEM: Uuid = uuid!("0FC63DAF-8483-4772-8E79-3D69D8477DE4");
    /// Linux swap.
    pub const LINUX_SWAP: Uuid = uuid!("0657FD6D-A4AB-43C4-84E5-0933C84B4F4F");
    /// Linux LVM.
    pub const LINUX_LVM: Uuid = uuid!("E6D6D379-F507-44C2-A23C-238F2A3DF928");
    /// Linux RAID.
    pub const LINUX_RAID: Uuid = uuid!("A19D880F-05FC-4D3B-A006-743F0F84911E");
    /// BIOS boot partition.
    pub const BIOS_BOOT: Uuid = uuid!("21686148-6449-6E6F-744E-656564454649");
    /// Apple HFS+.
    pub const APPLE_HFS: Uuid = uuid!("48465300-0000-11AA-AA11-00306543ECAC");
    /// Apple APFS container.
    pub const APPLE_APFS: Uuid = uuid!("7C3457EF-0000-11AA-AA11-00306543ECAC");
    /// Apple boot.
    pub const APPLE_BOOT: Uuid = uuid!("426F6F74-0000-11AA-AA11-00306543ECAC");
}

impl PartitionType {
    /// Human-readable description of the type.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            PartitionType::Mbr(t) => {
                mbr_type_name(*t).map_or_else(|| format!("type 0x{t:02X}"), str::to_owned)
            }
            PartitionType::Gpt(g) => gpt_type_name(g)
                .map_or_else(|| g.hyphenated().to_string().to_uppercase(), str::to_owned),
        }
    }

    /// Whether this is an MBR extended-partition container.
    #[must_use]
    pub const fn is_extended_container(&self) -> bool {
        matches!(self, PartitionType::Mbr(0x05 | 0x0F | 0x85))
    }

    /// Whether this is the GPT protective type.
    #[must_use]
    pub const fn is_gpt_protective(&self) -> bool {
        matches!(self, PartitionType::Mbr(0xEE))
    }
}

fn mbr_type_name(t: u8) -> Option<&'static str> {
    Some(match t {
        0x00 => "empty",
        0x01 => "FAT12",
        0x04 => "FAT16 (<32 MB)",
        0x05 => "Extended",
        0x06 => "FAT16",
        0x07 => "NTFS / exFAT / HPFS",
        0x0B => "FAT32 (CHS)",
        0x0C => "FAT32 (LBA)",
        0x0E => "FAT16 (LBA)",
        0x0F => "Extended (LBA)",
        0x11 => "Hidden FAT12",
        0x14 => "Hidden FAT16 (<32 MB)",
        0x16 => "Hidden FAT16",
        0x17 => "Hidden NTFS",
        0x1B => "Hidden FAT32",
        0x1C => "Hidden FAT32 (LBA)",
        0x27 => "Windows Recovery",
        0x82 => "Linux swap",
        0x83 => "Linux",
        0x85 => "Linux extended",
        0x8E => "Linux LVM",
        0xA5 => "FreeBSD",
        0xA6 => "OpenBSD",
        0xA8 => "Apple UFS",
        0xAB => "Apple boot",
        0xAF => "Apple HFS/HFS+",
        0xEE => "GPT protective",
        0xEF => "EFI System (FAT)",
        0xFD => "Linux RAID",
        _ => return None,
    })
}

fn gpt_type_name(g: &Uuid) -> Option<&'static str> {
    use gpt_types as t;
    Some(match *g {
        t::EFI_SYSTEM => "EFI System",
        t::MICROSOFT_RESERVED => "Microsoft Reserved",
        t::BASIC_DATA => "Basic Data",
        t::WINDOWS_RECOVERY => "Windows Recovery",
        t::LINUX_FILESYSTEM => "Linux filesystem",
        t::LINUX_SWAP => "Linux swap",
        t::LINUX_LVM => "Linux LVM",
        t::LINUX_RAID => "Linux RAID",
        t::BIOS_BOOT => "BIOS boot",
        t::APPLE_HFS => "Apple HFS+",
        t::APPLE_APFS => "Apple APFS",
        t::APPLE_BOOT => "Apple boot",
        _ => return None,
    })
}

bitflags! {
    /// Partition attribute flags normalised across schemes.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct PartitionFlags: u32 {
        /// MBR active/bootable flag (status 0x80).
        const BOOTABLE = 1 << 0;
        /// GPT bit 0: required by the platform.
        const GPT_REQUIRED = 1 << 1;
        /// GPT bit 1: no block I/O protocol.
        const GPT_NO_BLOCK_IO = 1 << 2;
        /// GPT bit 2: legacy BIOS bootable.
        const GPT_LEGACY_BIOS_BOOTABLE = 1 << 3;
        /// GPT basic-data bit 60: read-only.
        const READ_ONLY = 1 << 4;
        /// GPT basic-data bit 61: shadow copy.
        const SHADOW_COPY = 1 << 5;
        /// GPT basic-data bit 62: hidden.
        const HIDDEN = 1 << 6;
        /// GPT basic-data bit 63: no automatic mount.
        const NO_AUTOMOUNT = 1 << 7;
        /// The partition is a logical partition inside an MBR extended container.
        const LOGICAL = 1 << 8;
    }
}

/// How much the table itself vouches for a partition entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PartitionConfidence {
    /// The entry passed every structural check.
    High,
    /// The entry is usable but something about it is suspicious (overlap,
    /// odd status byte).
    Medium,
    /// The entry is partially outside the source or otherwise dubious.
    Low,
}

/// A partition entry normalised from MBR or GPT.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Partition {
    /// 1-based index in table order (MBR logical partitions continue at 5).
    pub index: u32,
    /// First logical sector.
    pub start_lba: u64,
    /// Last logical sector (inclusive).
    pub end_lba: u64,
    /// Byte offset of the first sector.
    pub start_offset: u64,
    /// Length in bytes.
    pub length: u64,
    /// Recorded type.
    pub partition_type: PartitionType,
    /// GPT partition name, if any.
    pub name: Option<String>,
    /// GPT unique partition GUID, if any.
    pub unique_guid: Option<Uuid>,
    /// Normalised flags.
    pub flags: PartitionFlags,
    /// Confidence in the entry.
    pub confidence: PartitionConfidence,
}

impl Partition {
    /// Builds a partition from an inclusive LBA range.
    ///
    /// # Errors
    ///
    /// Returns [`VolumeError::Overflow`] if the byte geometry overflows.
    pub fn from_lba(
        index: u32,
        start_lba: u64,
        end_lba: u64,
        sector_size: u32,
        partition_type: PartitionType,
    ) -> Result<Self, VolumeError> {
        if end_lba < start_lba {
            return Err(VolumeError::Overflow);
        }
        let sectors = arith::add(end_lba - start_lba, 1)?;
        let size = u64::from(sector_size);
        let start_offset = arith::mul(start_lba, size)?;
        let length = arith::mul(sectors, size)?;
        // The end must be representable.
        arith::add(start_offset, length)?;
        Ok(Self {
            index,
            start_lba,
            end_lba,
            start_offset,
            length,
            partition_type,
            name: None,
            unique_guid: None,
            flags: PartitionFlags::empty(),
            confidence: PartitionConfidence::High,
        })
    }

    /// The byte range covered by this partition.
    #[must_use]
    pub fn byte_range(&self) -> ByteRange {
        ByteRange::new(self.start_offset, self.length).unwrap_or(ByteRange {
            offset: self.start_offset,
            length: 0,
        })
    }

    /// Number of sectors.
    #[must_use]
    pub const fn sectors(&self) -> u64 {
        self.end_lba
            .saturating_sub(self.start_lba)
            .saturating_add(1)
    }

    /// Opens the partition as an independent reader over `parent`.
    ///
    /// If the partition extends beyond the source (a truncated image, say),
    /// the view is clamped to the bytes that exist.
    ///
    /// # Errors
    ///
    /// Returns [`VolumeError::Block`] if the partition starts beyond the end
    /// of the parent.
    pub fn open(&self, parent: Arc<dyn BlockReader>) -> Result<SubrangeReader, VolumeError> {
        let available = parent.len().checked_sub(self.start_offset).ok_or_else(|| {
            VolumeError::Block(phoinix_block::BlockError::OutOfBounds {
                offset: self.start_offset,
                length: self.length,
                source_len: parent.len(),
            })
        })?;
        let length = self.length.min(available);
        Ok(SubrangeReader::with_bounds(
            parent,
            self.start_offset,
            length,
        )?)
    }
}

/// The result of partition discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionTable {
    /// Scheme that was recognised.
    pub scheme: PartitionScheme,
    /// Logical sector size the LBAs are expressed in.
    pub sector_size: u32,
    /// Partitions in table order.
    pub partitions: Vec<Partition>,
    /// Findings, in the order they were made.
    pub diagnostics: Vec<VolumeDiagnostic>,
    /// GPT disk GUID, when a GPT was read.
    pub disk_guid: Option<Uuid>,
    /// MBR disk signature, when an MBR was read.
    pub mbr_disk_signature: Option<u32>,
}

impl PartitionTable {
    /// A table for a source with no partitioning.
    #[must_use]
    pub fn none(sector_size: u32, diagnostics: Vec<VolumeDiagnostic>) -> Self {
        Self {
            scheme: PartitionScheme::None,
            sector_size,
            partitions: Vec::new(),
            diagnostics,
            disk_guid: None,
            mbr_disk_signature: None,
        }
    }

    /// Checks every partition against the source length and against each
    /// other, appending diagnostics and lowering confidence where needed.
    pub fn validate_against(&mut self, source_len: u64) {
        let ranges: Vec<ByteRange> = self.partitions.iter().map(Partition::byte_range).collect();
        for (i, p) in self.partitions.iter_mut().enumerate() {
            let range = ranges.get(i).copied().unwrap_or(ByteRange {
                offset: 0,
                length: 0,
            });
            if range.end() > source_len || range.offset >= source_len {
                self.diagnostics
                    .push(VolumeDiagnostic::PartitionOutsideDevice { index: p.index });
                p.confidence = PartitionConfidence::Low;
            }
        }
        for i in 0..ranges.len() {
            for j in i + 1..ranges.len() {
                let (Some(a), Some(b)) = (ranges.get(i), ranges.get(j)) else {
                    continue;
                };
                if a.overlaps(b) {
                    let (fi, si) = (
                        self.partitions.get(i).map_or(0, |p| p.index),
                        self.partitions.get(j).map_or(0, |p| p.index),
                    );
                    // An extended container legitimately contains its logical partitions.
                    let container = self
                        .partitions
                        .get(i)
                        .is_some_and(|p| p.partition_type.is_extended_container())
                        || self
                            .partitions
                            .get(j)
                            .is_some_and(|p| p.partition_type.is_extended_container());
                    if container {
                        continue;
                    }
                    self.diagnostics
                        .push(VolumeDiagnostic::OverlappingPartitions {
                            first: fi,
                            second: si,
                        });
                    for k in [i, j] {
                        if let Some(p) = self.partitions.get_mut(k)
                            && p.confidence == PartitionConfidence::High
                        {
                            p.confidence = PartitionConfidence::Medium;
                        }
                    }
                }
            }
        }
    }

    /// Partitions that are not extended containers, i.e. those that may hold
    /// a filesystem.
    pub fn volumes(&self) -> impl Iterator<Item = &Partition> {
        self.partitions.iter().filter(|p| {
            !p.partition_type.is_extended_container() && !p.partition_type.is_gpt_protective()
        })
    }
}
