//! Block and inode bitmaps of the block groups.

use std::collections::HashMap;

use phoinix_block::{BlockReader, BlockReaderExt};

use crate::ExtError;
use crate::group::GroupDescriptor;
use crate::superblock::Superblock;

/// State of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockState {
    /// Free.
    Free,
    /// Allocated.
    Allocated,
    /// Outside the volume or unreadable.
    Unknown,
}

/// All block bitmaps, loaded lazily per group.
#[derive(Debug)]
pub struct BlockBitmaps {
    groups: Vec<GroupDescriptor>,
    cache: std::sync::Mutex<HashMap<u32, Option<Vec<u8>>>>,
}

impl BlockBitmaps {
    /// Prepares the bitmaps for `groups`.
    #[must_use]
    pub fn new(groups: Vec<GroupDescriptor>) -> Self {
        Self {
            groups,
            cache: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn bitmap(&self, reader: &dyn BlockReader, sb: &Superblock, group: u32) -> Option<Vec<u8>> {
        let mut cache = match self.cache.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(entry) = cache.get(&group) {
            return entry.clone();
        }
        let desc = self.groups.get(usize::try_from(group).ok()?)?;
        let loaded = if desc.block_uninit() {
            None
        } else {
            let len = usize::try_from(sb.block_size).ok()?;
            sb.block_offset(desc.block_bitmap)
                .ok()
                .and_then(|off| reader.read_vec(off, len).ok())
        };
        cache.insert(group, loaded.clone());
        loaded
    }

    /// State of `block`.
    #[must_use]
    pub fn state(&self, reader: &dyn BlockReader, sb: &Superblock, block: u64) -> BlockState {
        if block >= sb.blocks_count || block < u64::from(sb.first_data_block) {
            return BlockState::Unknown;
        }
        let rel = block - u64::from(sb.first_data_block);
        let group = u32::try_from(rel / u64::from(sb.blocks_per_group)).unwrap_or(u32::MAX);
        let index = usize::try_from(rel % u64::from(sb.blocks_per_group)).unwrap_or(usize::MAX);
        let Some(desc) = self
            .groups
            .get(usize::try_from(group).unwrap_or(usize::MAX))
        else {
            return BlockState::Unknown;
        };
        if desc.block_uninit() {
            return BlockState::Free;
        }
        match self.bitmap(reader, sb, group) {
            Some(bits) => match bits.get(index / 8) {
                Some(byte) if byte & (1 << (index % 8)) != 0 => BlockState::Allocated,
                Some(_) => BlockState::Free,
                None => BlockState::Unknown,
            },
            None => BlockState::Unknown,
        }
    }

    /// Whether the map could be read for `block`'s group.
    #[must_use]
    pub fn available(&self, reader: &dyn BlockReader, sb: &Superblock, block: u64) -> bool {
        self.state(reader, sb, block) != BlockState::Unknown
    }
}

/// Parses `bitmap_block` bytes; returns whether bit `index` is set.
#[must_use]
pub fn bit(bits: &[u8], index: usize) -> Option<bool> {
    bits.get(index / 8).map(|b| b & (1 << (index % 8)) != 0)
}

/// Reads the inode bitmap of `group`.
///
/// # Errors
///
/// Returns [`ExtError`] on read failures.
pub fn inode_bitmap(
    reader: &dyn BlockReader,
    sb: &Superblock,
    desc: &GroupDescriptor,
) -> Result<Option<Vec<u8>>, ExtError> {
    if desc.inode_uninit() {
        return Ok(None);
    }
    let len = usize::try_from(sb.block_size).map_err(|_| ExtError::Overflow)?;
    Ok(Some(
        reader.read_vec(sb.block_offset(desc.inode_bitmap)?, len)?,
    ))
}
