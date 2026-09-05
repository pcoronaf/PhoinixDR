//! Block group descriptors.

use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

/// Group flags.
pub mod flags {
    /// The inode bitmap is not initialised (all free).
    pub const INODE_UNINIT: u16 = 0x1;
    /// The block bitmap is not initialised (all free).
    pub const BLOCK_UNINIT: u16 = 0x2;
    /// The inode table is zeroed.
    pub const INODE_ZEROED: u16 = 0x4;
}

/// A block group descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDescriptor {
    /// Block bitmap block.
    pub block_bitmap: u64,
    /// Inode bitmap block.
    pub inode_bitmap: u64,
    /// First block of the inode table.
    pub inode_table: u64,
    /// Free blocks (as recorded).
    pub free_blocks: u32,
    /// Free inodes (as recorded).
    pub free_inodes: u32,
    /// Directories (as recorded).
    pub used_dirs: u32,
    /// Flags.
    pub flags: u16,
    /// Unused inodes at the end of the table.
    pub itable_unused: u32,
}

impl GroupDescriptor {
    /// Parses a descriptor of `desc_size` bytes.
    #[must_use]
    pub fn parse(bytes: &[u8], desc_size: u16) -> Option<Self> {
        let v = ByteView::new(bytes);
        let wide = desc_size >= 64 && bytes.len() >= 64;
        let hi = |off: usize| {
            if wide {
                u64::from(v.u32_le(off).unwrap_or(0)) << 32
            } else {
                0
            }
        };
        let hi16 = |off: usize| {
            if wide {
                u32::from(v.u16_le(off).unwrap_or(0)) << 16
            } else {
                0
            }
        };
        Some(Self {
            block_bitmap: u64::from(v.u32_le(0)?) | hi(32),
            inode_bitmap: u64::from(v.u32_le(4)?) | hi(36),
            inode_table: u64::from(v.u32_le(8)?) | hi(40),
            free_blocks: u32::from(v.u16_le(12)?) | hi16(44),
            free_inodes: u32::from(v.u16_le(14)?) | hi16(46),
            used_dirs: u32::from(v.u16_le(16)?) | hi16(48),
            flags: v.u16_le(18)?,
            itable_unused: u32::from(v.u16_le(28)?) | hi16(50),
        })
    }

    /// Whether the block bitmap is uninitialised (every block free).
    #[must_use]
    pub const fn block_uninit(&self) -> bool {
        self.flags & flags::BLOCK_UNINIT != 0
    }

    /// Whether the inode bitmap is uninitialised (every inode free).
    #[must_use]
    pub const fn inode_uninit(&self) -> bool {
        self.flags & flags::INODE_UNINIT != 0
    }
}
