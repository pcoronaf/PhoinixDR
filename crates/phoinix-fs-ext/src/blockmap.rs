//! ext2/ext3 block maps: 12 direct blocks, then indirect, double and
//! triple indirect blocks.

use std::collections::HashSet;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::bytes::ByteView;

use crate::ExtError;
use crate::extent::{MAX_EXTENTS, Run};
use crate::superblock::Superblock;

/// Callback appending a data block run to the output.
type PushRun<'a> = dyn FnMut(&mut Vec<Run>, u64, u64) -> Result<(), ExtError> + 'a;

/// Walks the block map in `i_block` for a file of `size` bytes, merging
/// contiguous blocks into runs and holes into hole runs.
///
/// # Errors
///
/// Returns [`ExtError::Malformed`] for pointers beyond the volume or
/// cyclic indirection.
pub fn walk(
    reader: &dyn BlockReader,
    sb: &Superblock,
    i_block: &[u8],
    size: u64,
) -> Result<Vec<Run>, ExtError> {
    let v = ByteView::new(i_block);
    let bs = u64::from(sb.block_size);
    let needed = size.div_ceil(bs.max(1));
    let mut out: Vec<Run> = Vec::new();
    let mut logical = 0u64;
    let mut visited: HashSet<u64> = HashSet::new();
    let per_block = bs / 4;
    let mut push = |out: &mut Vec<Run>, logical: u64, physical: u64| -> Result<(), ExtError> {
        let phys = (physical != 0).then_some(physical);
        if let Some(p) = phys
            && p >= sb.blocks_count
        {
            return Err(ExtError::Malformed {
                structure: "block map",
                detail: format!("block {p} beyond the volume"),
            });
        }
        match out.last_mut() {
            Some(last)
                if last.logical + last.count == logical
                    && match (last.physical, phys) {
                        (Some(a), Some(b)) => a + last.count == b,
                        (None, None) => true,
                        _ => false,
                    } =>
            {
                last.count += 1;
            }
            _ => out.push(Run {
                logical,
                physical: phys,
                count: 1,
                uninitialized: false,
            }),
        }
        if out.len() > MAX_EXTENTS {
            return Err(ExtError::Malformed {
                structure: "block map",
                detail: "too many extents".into(),
            });
        }
        Ok(())
    };
    // Direct blocks.
    for i in 0..12 {
        if logical >= needed {
            return Ok(out);
        }
        push(&mut out, logical, u64::from(v.u32_le(i * 4).unwrap_or(0)))?;
        logical += 1;
    }
    // Indirect levels.
    for (slot, level) in [(12usize, 1u32), (13, 2), (14, 3)] {
        if logical >= needed {
            break;
        }
        let ptr = u64::from(v.u32_le(slot * 4).unwrap_or(0));
        let span = per_block.saturating_pow(level);
        if ptr == 0 {
            // A hole covering the whole level.
            let n = span.min(needed.saturating_sub(logical));
            for _ in 0..n.min(1) {
                // Holes are merged; push once per block would be slow, so
                // push a single hole run of `n` blocks.
                out.push(Run {
                    logical,
                    physical: None,
                    count: n,
                    uninitialized: false,
                });
            }
            logical += n;
            continue;
        }
        walk_indirect(
            reader,
            sb,
            ptr,
            level,
            &mut logical,
            needed,
            &mut out,
            &mut visited,
            &mut push,
        )?;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn walk_indirect(
    reader: &dyn BlockReader,
    sb: &Superblock,
    block: u64,
    level: u32,
    logical: &mut u64,
    needed: u64,
    out: &mut Vec<Run>,
    visited: &mut HashSet<u64>,
    push: &mut PushRun<'_>,
) -> Result<(), ExtError> {
    if block >= sb.blocks_count || !visited.insert(block) {
        return Err(ExtError::Malformed {
            structure: "block map",
            detail: format!("indirect block {block} invalid or cyclic"),
        });
    }
    let bs = usize::try_from(sb.block_size).map_err(|_| ExtError::Overflow)?;
    let data = reader.read_vec(sb.block_offset(block)?, bs)?;
    let v = ByteView::new(&data);
    let per_block = u64::from(sb.block_size) / 4;
    let child_span = per_block.saturating_pow(level - 1);
    for i in 0..usize::try_from(per_block).unwrap_or(0) {
        if *logical >= needed {
            return Ok(());
        }
        let ptr = u64::from(v.u32_le(i * 4).unwrap_or(0));
        if level == 1 {
            push(out, *logical, ptr)?;
            *logical += 1;
        } else if ptr == 0 {
            let n = child_span.min(needed.saturating_sub(*logical));
            out.push(Run {
                logical: *logical,
                physical: None,
                count: n,
                uninitialized: false,
            });
            *logical += n;
        } else {
            walk_indirect(
                reader,
                sb,
                ptr,
                level - 1,
                logical,
                needed,
                out,
                visited,
                push,
            )?;
        }
    }
    Ok(())
}
