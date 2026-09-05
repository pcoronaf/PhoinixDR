//! The ext superblock (1024 bytes at byte offset 1024).

use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

use crate::ExtError;

/// Superblock magic.
pub const MAGIC: u16 = 0xEF53;
/// Byte offset of the primary superblock.
pub const SUPERBLOCK_OFFSET: u64 = 1024;

/// Compatible features.
pub mod compat {
    /// The filesystem has a journal.
    pub const HAS_JOURNAL: u32 = 0x0004;
    /// Directory indexing (htree).
    pub const DIR_INDEX: u32 = 0x0020;
}

/// Incompatible features.
pub mod incompat {
    /// Directory entries carry a file type.
    pub const FILETYPE: u32 = 0x0002;
    /// The journal needs recovery.
    pub const RECOVER: u32 = 0x0004;
    /// Group descriptors spread in meta block groups.
    pub const META_BG: u32 = 0x0010;
    /// Extents.
    pub const EXTENTS: u32 = 0x0040;
    /// 64-bit block numbers.
    pub const BIT64: u32 = 0x0080;
    /// Flexible block groups.
    pub const FLEX_BG: u32 = 0x0200;
    /// Checksum seed stored in the superblock.
    pub const CSUM_SEED: u32 = 0x2000;
    /// Inline data.
    pub const INLINE_DATA: u32 = 0x8000;
}

/// Read-only compatible features.
pub mod ro_compat {
    /// Sparse superblock backups.
    pub const SPARSE_SUPER: u32 = 0x0001;
    /// Group descriptor checksums (crc16).
    pub const GDT_CSUM: u32 = 0x0010;
    /// Extra inode fields.
    pub const EXTRA_ISIZE: u32 = 0x0040;
    /// Metadata checksums (crc32c).
    pub const METADATA_CSUM: u32 = 0x0400;
}

/// A parsed superblock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Superblock {
    /// Total inodes.
    pub inodes_count: u32,
    /// Total blocks.
    pub blocks_count: u64,
    /// Free blocks (as recorded).
    pub free_blocks: u64,
    /// Free inodes (as recorded).
    pub free_inodes: u32,
    /// First data block (1 for 1 KiB blocks, else 0).
    pub first_data_block: u32,
    /// Block size in bytes.
    pub block_size: u32,
    /// Blocks per group.
    pub blocks_per_group: u32,
    /// Inodes per group.
    pub inodes_per_group: u32,
    /// Filesystem state.
    pub state: u16,
    /// Revision.
    pub rev_level: u32,
    /// First non-reserved inode.
    pub first_ino: u32,
    /// Inode size in bytes.
    pub inode_size: u16,
    /// Compatible features.
    pub feature_compat: u32,
    /// Incompatible features.
    pub feature_incompat: u32,
    /// Read-only compatible features.
    pub feature_ro_compat: u32,
    /// Filesystem UUID.
    pub uuid: [u8; 16],
    /// Volume label.
    pub volume_name: String,
    /// Last mount point.
    pub last_mounted: String,
    /// Journal inode number (0 = none).
    pub journal_inum: u32,
    /// Group descriptor size (32 or 64).
    pub desc_size: u16,
    /// First meta block group.
    pub first_meta_bg: u32,
    /// Checksum seed for metadata checksums.
    pub csum_seed: u32,
    /// Checksum type (1 = crc32c).
    pub checksum_type: u8,
    /// Stored superblock checksum.
    pub checksum: u32,
    /// Whether the stored checksum matches (None when not checksummed).
    pub checksum_ok: Option<bool>,
    /// Number of the block group this copy belongs to.
    pub block_group_nr: u16,
    /// Last mount time (Unix seconds).
    pub mtime: u32,
    /// Last write time (Unix seconds).
    pub wtime: u32,
}

impl Superblock {
    /// Parses a 1024-byte superblock.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError::InvalidSuperblock`] when the magic or the
    /// geometry is invalid.
    pub fn parse(bytes: &[u8]) -> Result<Self, ExtError> {
        let v = ByteView::new(bytes);
        let invalid = |what: &str| ExtError::InvalidSuperblock(what.to_owned());
        if bytes.len() < 1024 {
            return Err(invalid("shorter than 1024 bytes"));
        }
        if v.u16_le(56) != Some(MAGIC) {
            return Err(invalid("magic 0xEF53 absent"));
        }
        let log_block = v.u32_le(24).unwrap_or(u32::MAX);
        if log_block > 6 {
            return Err(invalid("block size larger than 64 KiB"));
        }
        let block_size = 1024u32 << log_block;
        let feature_incompat = v.u32_le(96).unwrap_or(0);
        let feature_ro_compat = v.u32_le(100).unwrap_or(0);
        let feature_compat = v.u32_le(92).unwrap_or(0);
        let mut blocks_count = u64::from(v.u32_le(4).unwrap_or(0));
        let mut free_blocks = u64::from(v.u32_le(12).unwrap_or(0));
        if feature_incompat & incompat::BIT64 != 0 {
            blocks_count |= u64::from(v.u32_le(0x150).unwrap_or(0)) << 32;
            free_blocks |= u64::from(v.u32_le(0x158).unwrap_or(0)) << 32;
        }
        let rev_level = v.u32_le(76).unwrap_or(0);
        let (first_ino, inode_size) = if rev_level == 0 {
            (11, 128)
        } else {
            (v.u32_le(84).unwrap_or(11), v.u16_le(88).unwrap_or(128))
        };
        if !inode_size.is_power_of_two() || inode_size < 128 || u32::from(inode_size) > block_size {
            return Err(invalid(
                "inode size is not a power of two between 128 and the block size",
            ));
        }
        let inodes_count = v.u32_le(0).unwrap_or(0);
        let blocks_per_group = v.u32_le(32).unwrap_or(0);
        let inodes_per_group = v.u32_le(40).unwrap_or(0);
        if inodes_count == 0 || blocks_count == 0 || blocks_per_group == 0 || inodes_per_group == 0
        {
            return Err(invalid("zero inode or block counts"));
        }
        if blocks_per_group > block_size.saturating_mul(8) {
            return Err(invalid(
                "more blocks per group than the block bitmap can describe",
            ));
        }
        let desc_size = if feature_incompat & incompat::BIT64 != 0 {
            let d = v.u16_le(254).unwrap_or(32);
            if d < 32 || !d.is_power_of_two() {
                return Err(invalid("group descriptor size is invalid"));
            }
            d
        } else {
            32
        };
        let uuid_bytes = v.slice(104, 16).unwrap_or(&[0u8; 16]);
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(uuid_bytes);
        let text = |off: usize, len: usize| {
            v.slice(off, len)
                .map(|b| String::from_utf8_lossy(b).trim_end_matches('\0').to_owned())
                .unwrap_or_default()
        };
        let checksum_type = v
            .slice(0x175, 1)
            .and_then(|b| b.first().copied())
            .unwrap_or(0);
        let checksum = v.u32_le(0x3FC).unwrap_or(0);
        let checksum_ok = if feature_ro_compat & ro_compat::METADATA_CSUM != 0 {
            let head = bytes.get(..0x3FC).unwrap_or(&[]);
            Some(phoinix_core::crc32c::update(!0, head) == checksum)
        } else {
            None
        };
        let csum_seed = if feature_incompat & incompat::CSUM_SEED != 0 {
            v.u32_le(0x270).unwrap_or(0)
        } else {
            phoinix_core::crc32c::update(!0, &uuid)
        };
        Ok(Self {
            inodes_count,
            blocks_count,
            free_blocks,
            free_inodes: v.u32_le(16).unwrap_or(0),
            first_data_block: v.u32_le(20).unwrap_or(0),
            block_size,
            blocks_per_group,
            inodes_per_group,
            state: v.u16_le(58).unwrap_or(0),
            rev_level,
            first_ino,
            inode_size,
            feature_compat,
            feature_incompat,
            feature_ro_compat,
            uuid,
            volume_name: text(120, 16),
            last_mounted: text(136, 64),
            journal_inum: v.u32_le(224).unwrap_or(0),
            desc_size,
            first_meta_bg: v.u32_le(0x104).unwrap_or(0),
            csum_seed,
            checksum_type,
            checksum,
            checksum_ok,
            block_group_nr: v.u16_le(90).unwrap_or(0),
            mtime: v.u32_le(44).unwrap_or(0),
            wtime: v.u32_le(48).unwrap_or(0),
        })
    }

    /// Whether `feature` (incompatible) is set.
    #[must_use]
    pub const fn has_incompat(&self, feature: u32) -> bool {
        self.feature_incompat & feature != 0
    }

    /// Whether `feature` (read-only compatible) is set.
    #[must_use]
    pub const fn has_ro_compat(&self, feature: u32) -> bool {
        self.feature_ro_compat & feature != 0
    }

    /// Whether `feature` (compatible) is set.
    #[must_use]
    pub const fn has_compat(&self, feature: u32) -> bool {
        self.feature_compat & feature != 0
    }

    /// Whether metadata checksums are in use.
    #[must_use]
    pub const fn metadata_csum(&self) -> bool {
        self.has_ro_compat(ro_compat::METADATA_CSUM)
    }

    /// Number of block groups.
    #[must_use]
    pub fn group_count(&self) -> u32 {
        let data_blocks = self
            .blocks_count
            .saturating_sub(u64::from(self.first_data_block));
        u32::try_from(data_blocks.div_ceil(u64::from(self.blocks_per_group.max(1))))
            .unwrap_or(u32::MAX)
    }

    /// The flavour name (`ext2`, `ext3`, `ext4`).
    #[must_use]
    pub fn flavour(&self) -> &'static str {
        if self.has_incompat(incompat::EXTENTS)
            || self.has_incompat(incompat::BIT64)
            || self.has_incompat(incompat::FLEX_BG)
        {
            "ext4"
        } else if self.has_compat(compat::HAS_JOURNAL) {
            "ext3"
        } else {
            "ext2"
        }
    }

    /// Byte offset of a block.
    ///
    /// # Errors
    ///
    /// Returns [`ExtError::Overflow`] when the offset does not fit.
    pub fn block_offset(&self, block: u64) -> Result<u64, ExtError> {
        block
            .checked_mul(u64::from(self.block_size))
            .ok_or(ExtError::Overflow)
    }

    /// Whether `group` carries a superblock backup.
    #[must_use]
    pub fn group_has_super(&self, group: u32) -> bool {
        if !self.has_ro_compat(ro_compat::SPARSE_SUPER) || group <= 1 {
            return true;
        }
        if group % 2 == 0 {
            return false;
        }
        let is_power = |base: u32| {
            let mut n = base;
            while n < group {
                n = n.saturating_mul(base);
            }
            n == group
        };
        is_power(3) || is_power(5) || is_power(7)
    }

    /// Block number where the group descriptor of `group` lives, and the
    /// byte offset of the descriptor inside that block.
    #[must_use]
    pub fn descriptor_location(&self, group: u32) -> (u64, u64) {
        let per_block = u64::from(self.block_size) / u64::from(self.desc_size.max(32));
        let per_block = per_block.max(1);
        let g = u64::from(group);
        let within = (g % per_block) * u64::from(self.desc_size);
        let meta_bg =
            self.has_incompat(incompat::META_BG) && g >= u64::from(self.first_meta_bg) * per_block;
        if meta_bg {
            let first_group = g - g % per_block;
            let first_block =
                u64::from(self.first_data_block) + first_group * u64::from(self.blocks_per_group);
            let has_super = u32::try_from(first_group).is_ok_and(|fg| self.group_has_super(fg));
            (first_block + u64::from(has_super), within)
        } else {
            (u64::from(self.first_data_block) + 1 + g / per_block, within)
        }
    }
}
