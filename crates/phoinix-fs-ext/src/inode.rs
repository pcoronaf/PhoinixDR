//! Inodes: the 128-byte base, the extra fields, checksums and the layout
//! description held in `i_block`.

use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

use crate::superblock::Superblock;

/// Inode flags.
pub mod flags {
    /// Directory uses an htree index.
    pub const INDEX: u32 = 0x0000_1000;
    /// Block counts are in filesystem blocks.
    pub const HUGE_FILE: u32 = 0x0004_0000;
    /// The inode uses extents.
    pub const EXTENTS: u32 = 0x0008_0000;
    /// The data lives inside the inode.
    pub const INLINE_DATA: u32 = 0x1000_0000;
}

/// File type bits of `i_mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InodeKind {
    /// Regular file.
    Regular,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
    /// Something else (device, socket, fifo).
    Other,
    /// Mode is zero: never used or wiped.
    Unused,
}

/// A parsed inode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Inode {
    /// Inode number.
    pub number: u32,
    /// Mode bits.
    pub mode: u16,
    /// Owner.
    pub uid: u32,
    /// Group.
    pub gid: u32,
    /// Size in bytes.
    pub size: u64,
    /// Access time.
    pub atime: u32,
    /// Change time.
    pub ctime: u32,
    /// Modification time.
    pub mtime: u32,
    /// Deletion time (0 = not deleted).
    pub dtime: u32,
    /// Creation time (ext4 extra field), if present.
    pub crtime: Option<u32>,
    /// Hard links.
    pub links: u16,
    /// Blocks allocated, in 512-byte units unless `HUGE_FILE`.
    pub blocks: u64,
    /// Flags.
    pub flags: u32,
    /// Generation.
    pub generation: u32,
    /// The 60 bytes of `i_block`.
    pub i_block: Vec<u8>,
    /// Extra inode size, when the inode is larger than 128 bytes.
    pub extra_isize: u16,
    /// Whether the checksum matched (None when not checksummed).
    pub checksum_ok: Option<bool>,
}

impl Inode {
    /// Parses inode `number` from its on-disk bytes (`inode_size` long).
    #[must_use]
    pub fn parse(number: u32, bytes: &[u8], sb: &Superblock) -> Option<Self> {
        let v = ByteView::new(bytes);
        if bytes.len() < 128 {
            return None;
        }
        let mode = v.u16_le(0)?;
        let osd2_uid_high = u32::from(v.u16_le(120)?) << 16;
        let osd2_gid_high = u32::from(v.u16_le(122)?) << 16;
        let mut size = u64::from(v.u32_le(4)?);
        let kind_dir = mode & 0xF000 == 0x4000;
        if !kind_dir
            || sb.has_ro_compat(crate::superblock::ro_compat::EXTRA_ISIZE)
            || sb.rev_level > 0
        {
            size |= u64::from(v.u32_le(108)?) << 32;
        }
        let flags = v.u32_le(32)?;
        let mut blocks = u64::from(v.u32_le(28)?);
        if sb.has_ro_compat(0x0008) {
            blocks |= u64::from(v.u16_le(116)?) << 32;
        }
        let i_block = v.slice(40, 60)?.to_vec();
        let extra_isize = if bytes.len() > 128 {
            v.u16_le(128).unwrap_or(0)
        } else {
            0
        };
        let crtime = if extra_isize >= 24 {
            v.u32_le(144)
        } else {
            None
        };
        let checksum_ok = if sb.metadata_csum() {
            Some(verify_checksum(number, bytes, sb, extra_isize))
        } else {
            None
        };
        Some(Self {
            number,
            mode,
            uid: u32::from(v.u16_le(2)?) | osd2_uid_high,
            gid: u32::from(v.u16_le(24)?) | osd2_gid_high,
            size,
            atime: v.u32_le(8)?,
            ctime: v.u32_le(12)?,
            mtime: v.u32_le(16)?,
            dtime: v.u32_le(20)?,
            crtime,
            links: v.u16_le(26)?,
            blocks,
            flags,
            generation: v.u32_le(100)?,
            i_block,
            extra_isize,
            checksum_ok,
        })
    }

    /// The kind of object.
    #[must_use]
    pub const fn kind(&self) -> InodeKind {
        match self.mode & 0xF000 {
            0 => InodeKind::Unused,
            0x8000 => InodeKind::Regular,
            0x4000 => InodeKind::Directory,
            0xA000 => InodeKind::Symlink,
            _ => InodeKind::Other,
        }
    }

    /// Whether the inode uses extents.
    #[must_use]
    pub const fn uses_extents(&self) -> bool {
        self.flags & flags::EXTENTS != 0
    }

    /// Whether the data lives inline.
    #[must_use]
    pub const fn inline_data(&self) -> bool {
        self.flags & flags::INLINE_DATA != 0
    }

    /// Whether the inode is deleted (no links, or a deletion time).
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.dtime != 0 || (self.links == 0 && self.mode != 0)
    }

    /// Whether the inode describes any data blocks.
    #[must_use]
    pub fn has_layout(&self) -> bool {
        if self.inline_data() {
            return true;
        }
        if self.uses_extents() {
            let v = ByteView::new(&self.i_block);
            v.u16_le(0) == Some(crate::extent::MAGIC) && v.u16_le(2).unwrap_or(0) > 0
        } else {
            self.i_block
                .chunks_exact(4)
                .take(15)
                .any(|c| c != [0, 0, 0, 0])
        }
    }
}

/// Verifies the crc32c inode checksum.
fn verify_checksum(number: u32, bytes: &[u8], sb: &Superblock, extra_isize: u16) -> bool {
    let v = ByteView::new(bytes);
    let lo = v.u16_le(124).unwrap_or(0);
    let has_hi = bytes.len() > 128 && extra_isize >= 4;
    let hi = if has_hi {
        v.u16_le(130).unwrap_or(0)
    } else {
        0
    };
    let generation = v.u32_le(100).unwrap_or(0);
    let mut crc = crate::crc32c::update(sb.csum_seed, &number.to_le_bytes());
    crc = crate::crc32c::update(crc, &generation.to_le_bytes());
    let mut copy = bytes.to_vec();
    if let Some(s) = copy.get_mut(124..126) {
        s.copy_from_slice(&[0, 0]);
    }
    if has_hi && let Some(s) = copy.get_mut(130..132) {
        s.copy_from_slice(&[0, 0]);
    }
    crc = crate::crc32c::update(crc, &copy);
    if has_hi {
        crc == (u32::from(hi) << 16 | u32::from(lo))
    } else {
        (crc & 0xFFFF) == u32::from(lo)
    }
}
