//! ext4 extent trees.

use std::collections::HashSet;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::bytes::ByteView;
use serde::{Deserialize, Serialize};

use crate::ExtError;
use crate::superblock::Superblock;

/// Extent header magic.
pub const MAGIC: u16 = 0xF30A;
/// Deepest tree walked.
pub const MAX_DEPTH: u16 = 5;
/// Most extents collected for one file.
pub const MAX_EXTENTS: usize = 1_000_000;

/// One mapped run of a file: logical block, physical block, count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    /// First logical block.
    pub logical: u64,
    /// First physical block, or `None` for a hole.
    pub physical: Option<u64>,
    /// Number of blocks.
    pub count: u64,
    /// Extent is uninitialised (reads as zeros).
    pub uninitialized: bool,
}

/// Walks an extent tree rooted in `root` (60 bytes of `i_block` or a
/// tree block), collecting runs in logical order.
///
/// # Errors
///
/// Returns [`ExtError::Malformed`] for a bad header or a cyclic tree.
pub fn walk(reader: &dyn BlockReader, sb: &Superblock, root: &[u8]) -> Result<Vec<Run>, ExtError> {
    let mut runs = Vec::new();
    let mut visited: HashSet<u64> = HashSet::new();
    walk_node(reader, sb, root, 0, &mut runs, &mut visited)?;
    runs.sort_by_key(|r| r.logical);
    Ok(runs)
}

fn walk_node(
    reader: &dyn BlockReader,
    sb: &Superblock,
    node: &[u8],
    level: u16,
    runs: &mut Vec<Run>,
    visited: &mut HashSet<u64>,
) -> Result<(), ExtError> {
    let v = ByteView::new(node);
    let malformed = |detail: String| ExtError::Malformed {
        structure: "extent tree",
        detail,
    };
    if v.u16_le(0) != Some(MAGIC) {
        return Err(malformed("header magic absent".into()));
    }
    let entries = v.u16_le(2).unwrap_or(0);
    let max = v.u16_le(4).unwrap_or(0);
    let depth = v.u16_le(6).unwrap_or(0);
    if entries > max || depth > MAX_DEPTH || level > MAX_DEPTH {
        return Err(malformed(format!(
            "{entries} entries of {max}, depth {depth}"
        )));
    }
    let capacity = (node.len().saturating_sub(12)) / 12;
    let entries = usize::from(entries).min(capacity);
    for i in 0..entries {
        let off = 12 + i * 12;
        if depth == 0 {
            let logical = u64::from(v.u32_le(off).unwrap_or(0));
            let raw_len = v.u16_le(off + 4).unwrap_or(0);
            let (count, uninitialized) = if raw_len > 32768 {
                (u64::from(raw_len - 32768), true)
            } else {
                (u64::from(raw_len), false)
            };
            let physical = u64::from(v.u32_le(off + 8).unwrap_or(0))
                | (u64::from(v.u16_le(off + 6).unwrap_or(0)) << 32);
            if count == 0 {
                continue;
            }
            if physical >= sb.blocks_count {
                return Err(malformed(format!(
                    "extent at block {physical} beyond the volume"
                )));
            }
            runs.push(Run {
                logical,
                physical: Some(physical),
                count,
                uninitialized,
            });
            if runs.len() > MAX_EXTENTS {
                return Err(malformed("too many extents".into()));
            }
        } else {
            let leaf = u64::from(v.u32_le(off + 4).unwrap_or(0))
                | (u64::from(v.u16_le(off + 8).unwrap_or(0)) << 32);
            if leaf >= sb.blocks_count {
                return Err(malformed(format!(
                    "index at block {leaf} beyond the volume"
                )));
            }
            if !visited.insert(leaf) {
                return Err(malformed("cyclic extent tree".into()));
            }
            let block = reader.read_vec(
                sb.block_offset(leaf)?,
                usize::try_from(sb.block_size).map_err(|_| ExtError::Overflow)?,
            )?;
            walk_node(reader, sb, &block, level + 1, runs, visited)?;
        }
    }
    Ok(())
}
