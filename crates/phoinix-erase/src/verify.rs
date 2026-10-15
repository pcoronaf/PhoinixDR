//! Read-back verification through the read-only stack.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::ByteRange;

use crate::certificate::VerificationRecord;
use crate::error::EraseError;
use crate::method::{PassPattern, VerifyMode};
use crate::pattern::{CHUNK_BYTES, Seed, fill_chunk};
use crate::target::TargetDescription;

/// Block size of a verification sample.
pub const SAMPLE_BLOCK: usize = 64 * 1024;
/// Blocks sampled on a large target.
pub const SAMPLE_BLOCKS: u64 = 4096;
/// Targets up to this size are verified in full even in sample mode.
pub const FULL_UP_TO: u64 = 1024 * 1024 * 1024;

/// Opens the target read-only through the device layer.
///
/// # Errors
///
/// Returns the device error.
pub fn open_reader(desc: &TargetDescription) -> Result<Arc<dyn BlockReader>, EraseError> {
    Ok(phoinix_device::open_source(desc.target.path())?)
}

/// What to read back and compare.
#[derive(Debug, Clone, Copy)]
pub struct ReadBack<'a> {
    /// The target.
    pub desc: &'a TargetDescription,
    /// The last pass's pattern.
    pub last: &'a PassPattern,
    /// The run's seed.
    pub seed: &'a Seed,
    /// Index of the last pass.
    pub pass_index: usize,
    /// How much to check.
    pub mode: VerifyMode,
    /// Ranges the device refused to write (skipped, not counted).
    pub failed: &'a [ByteRange],
}

/// Reads the target back and compares it with the last pass.
///
/// # Errors
///
/// Returns [`EraseError`] when the target cannot be read at all;
/// individual read failures count as mismatches.
pub fn read_back(
    plan: &ReadBack<'_>,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<VerificationRecord, EraseError> {
    let ReadBack {
        desc,
        last,
        seed,
        pass_index,
        mode,
        failed,
    } = *plan;
    let mut record = VerificationRecord {
        mode,
        ..Default::default()
    };
    if mode == VerifyMode::None || desc.length == 0 {
        return Ok(record);
    }
    let reader = open_reader(desc)?;
    let full = mode == VerifyMode::Full || desc.length <= FULL_UP_TO;
    let block = SAMPLE_BLOCK as u64;
    let blocks_total = desc.length.div_ceil(block);
    let samples = if full {
        blocks_total
    } else {
        blocks_total.min(SAMPLE_BLOCKS)
    };
    let mut expected = vec![0u8; CHUNK_BYTES];
    let mut expected_chunk: Option<u64> = None;
    let mut buf = vec![0u8; SAMPLE_BLOCK];
    for i in 0..samples {
        if cancel.load(Ordering::Relaxed) {
            record.cancelled = true;
            break;
        }
        let index = if samples == blocks_total {
            i
        } else {
            i * blocks_total / samples
        };
        let rel = index * block;
        let want = usize::try_from((desc.length - rel).min(block)).unwrap_or(SAMPLE_BLOCK);
        let abs = desc.offset + rel;
        if failed
            .iter()
            .any(|f| f.offset < abs + want as u64 && f.end() > abs)
        {
            record.skipped_unwritable_bytes += want as u64;
            continue;
        }
        let chunk = rel / CHUNK_BYTES as u64;
        if expected_chunk != Some(chunk) {
            fill_chunk(last, seed, pass_index, chunk, &mut expected);
            expected_chunk = Some(chunk);
        }
        let within = usize::try_from(rel % CHUNK_BYTES as u64).unwrap_or(0);
        let slot = buf.get_mut(..want).unwrap_or(&mut []);
        let ok = reader.read_exact_at(abs, slot).is_ok();
        record.blocks_checked += 1;
        record.bytes_checked += want as u64;
        let matches = ok && expected.get(within..within + want) == Some(&*slot);
        if !matches {
            record.blocks_mismatched += 1;
            if record.first_mismatch.is_none() {
                record.first_mismatch = Some(abs);
            }
        }
        if i % 64 == 0 {
            progress(i, samples);
        }
    }
    progress(samples, samples);
    record.passed = !record.cancelled && record.blocks_mismatched == 0 && record.blocks_checked > 0;
    Ok(record)
}
