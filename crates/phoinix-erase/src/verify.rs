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

/// Maps a position in the concatenated spans of a target to its absolute
/// offset on the path.
#[derive(Debug, Clone)]
pub struct SpanMap {
    spans: Vec<ByteRange>,
    /// Cumulative start (relative) of each span.
    starts: Vec<u64>,
    total: u64,
}

impl SpanMap {
    /// Builds the map; empty spans are dropped.
    #[must_use]
    pub fn new(spans: &[ByteRange]) -> Self {
        let mut starts = Vec::with_capacity(spans.len());
        let mut kept = Vec::with_capacity(spans.len());
        let mut total = 0u64;
        for s in spans {
            if s.length == 0 {
                continue;
            }
            starts.push(total);
            kept.push(*s);
            total = total.saturating_add(s.length);
        }
        Self {
            spans: kept,
            starts,
            total,
        }
    }

    /// Bytes in every span.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// Number of `block`-sized pieces when every span is cut into pieces
    /// of at most `block` bytes (a piece never crosses a span).
    #[must_use]
    pub fn pieces(&self, block: u64) -> u64 {
        let block = block.max(1);
        self.spans.iter().map(|s| s.length.div_ceil(block)).sum()
    }

    /// The `index`-th piece of [`pieces`](Self::pieces): its relative
    /// position, absolute offset and length. `None` past the end.
    #[must_use]
    pub fn piece(&self, block: u64, index: u64) -> Option<(u64, u64, u64)> {
        let block = block.max(1);
        let mut skipped = 0u64;
        for (span, start) in self.spans.iter().zip(&self.starts) {
            let n = span.length.div_ceil(block);
            if index < skipped + n {
                let k = index - skipped;
                let within = k * block;
                let len = (span.length - within).min(block);
                return Some((start + within, span.offset + within, len));
            }
            skipped += n;
        }
        None
    }

    /// The absolute offset of relative position `rel` and the bytes left in
    /// its span. Past the end: the end of the last span and 0.
    #[must_use]
    pub fn locate(&self, rel: u64) -> (u64, u64) {
        if self.spans.is_empty() {
            return (0, 0);
        }
        let i = self.starts.partition_point(|s| *s <= rel).saturating_sub(1);
        let (Some(span), Some(start)) = (self.spans.get(i), self.starts.get(i)) else {
            return (0, 0);
        };
        let within = rel.saturating_sub(*start).min(span.length);
        (span.offset + within, span.length - within)
    }
}

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
    let spans = SpanMap::new(&desc.spans());
    let total = spans.total();
    let full = mode == VerifyMode::Full || total <= FULL_UP_TO;
    let block = SAMPLE_BLOCK as u64;
    let blocks_total = spans.pieces(block);
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
        // Pieces never cross a span, so a full verification reads every
        // byte of every range.
        let Some((rel, abs, len)) = spans.piece(block, index) else {
            break;
        };
        let want = usize::try_from(len).unwrap_or(SAMPLE_BLOCK);
        if want == 0 {
            continue;
        }
        if failed
            .iter()
            .any(|f| f.offset < abs + want as u64 && f.end() > abs)
        {
            record.skipped_unwritable_bytes += want as u64;
            continue;
        }
        let slot = buf.get_mut(..want).unwrap_or(&mut []);
        let ok = reader.read_exact_at(abs, slot).is_ok();
        record.blocks_checked += 1;
        record.bytes_checked += want as u64;
        // A piece may straddle a chunk boundary: compare it in parts.
        let mut matches = ok;
        let mut done = 0usize;
        while matches && done < want {
            let pos = rel + done as u64;
            let chunk = pos / CHUNK_BYTES as u64;
            if expected_chunk != Some(chunk) {
                fill_chunk(last, seed, pass_index, chunk, &mut expected);
                expected_chunk = Some(chunk);
            }
            let within = usize::try_from(pos % CHUNK_BYTES as u64).unwrap_or(0);
            let n = (CHUNK_BYTES - within).min(want - done);
            matches = expected.get(within..within + n) == slot.get(done..done + n);
            done += n;
        }
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

/// What to read back through the filesystem: files (or named streams)
/// and their lengths, in the order they were written.
#[derive(Debug, Clone, Copy)]
pub struct ReadBackPaths<'a> {
    /// Paths and the number of bytes written to each.
    pub items: &'a [(std::path::PathBuf, u64)],
    /// The last pass's pattern.
    pub last: &'a PassPattern,
    /// The run's seed.
    pub seed: &'a Seed,
    /// Index of the last pass.
    pub pass_index: usize,
    /// How much to check.
    pub mode: VerifyMode,
}

/// Reads files written through the filesystem back and compares them with
/// the last pass; the relative position runs across every item in order,
/// exactly as the writer counted it.
pub fn read_back_paths(
    plan: &ReadBackPaths<'_>,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> VerificationRecord {
    use std::io::{Read, Seek, SeekFrom};
    let ReadBackPaths {
        items,
        last,
        seed,
        pass_index,
        mode,
    } = *plan;
    let mut record = VerificationRecord {
        mode,
        ..Default::default()
    };
    let total: u64 = items.iter().map(|(_, n)| *n).sum();
    if mode == VerifyMode::None || total == 0 {
        return record;
    }
    // Every item is cut into pieces of at most one sample block, so that a
    // full verification reads every byte of every item.
    let spans: Vec<ByteRange> = {
        let mut rel = 0u64;
        items
            .iter()
            .map(|(_, n)| {
                let r = ByteRange {
                    offset: rel,
                    length: *n,
                };
                rel += *n;
                r
            })
            .collect()
    };
    let map = SpanMap::new(&spans);
    let full = mode == VerifyMode::Full || total <= FULL_UP_TO;
    let block = SAMPLE_BLOCK as u64;
    let blocks_total = map.pieces(block);
    let samples = if full {
        blocks_total
    } else {
        blocks_total.min(SAMPLE_BLOCKS)
    };
    let mut expected = vec![0u8; CHUNK_BYTES];
    let mut expected_chunk: Option<u64> = None;
    let mut buf = vec![0u8; SAMPLE_BLOCK];
    let mut open: Option<(usize, std::fs::File)> = None;
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
        let Some((rel, _, len)) = map.piece(block, index) else {
            break;
        };
        let want = usize::try_from(len).unwrap_or(SAMPLE_BLOCK);
        if want == 0 {
            continue;
        }
        // Locate the item holding `rel`.
        let mut start = 0u64;
        let mut which = 0usize;
        for (fi, (_, size)) in items.iter().enumerate() {
            if rel < start + size {
                which = fi;
                break;
            }
            start += size;
        }
        let in_file = rel - start;
        if open.as_ref().is_none_or(|(fi, _)| *fi != which) {
            open = items
                .get(which)
                .and_then(|(path, _)| std::fs::File::open(path).ok())
                .map(|f| (which, f));
        }
        let slot = buf.get_mut(..want).unwrap_or(&mut []);
        let ok = open.as_mut().is_some_and(|(_, f)| {
            f.seek(SeekFrom::Start(in_file)).is_ok() && f.read_exact(slot).is_ok()
        });
        record.blocks_checked += 1;
        record.bytes_checked += want as u64;
        // A piece may straddle a chunk boundary: compare it in parts.
        let mut matches = ok;
        let mut done = 0usize;
        while matches && done < want {
            let pos = rel + done as u64;
            let chunk = pos / CHUNK_BYTES as u64;
            if expected_chunk != Some(chunk) {
                fill_chunk(last, seed, pass_index, chunk, &mut expected);
                expected_chunk = Some(chunk);
            }
            let within = usize::try_from(pos % CHUNK_BYTES as u64).unwrap_or(0);
            let n = (CHUNK_BYTES - within).min(want - done);
            matches = expected.get(within..within + n) == slot.get(done..done + n);
            done += n;
        }
        if !matches {
            record.blocks_mismatched += 1;
            if record.first_mismatch.is_none() {
                record.first_mismatch = Some(rel);
            }
        }
        if i % 64 == 0 {
            progress(i, samples);
        }
    }
    progress(samples, samples);
    record.passed = !record.cancelled && record.blocks_mismatched == 0 && record.blocks_checked > 0;
    record
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_map_locates_positions_across_gaps() {
        let m = SpanMap::new(&[
            ByteRange {
                offset: 1000,
                length: 10,
            },
            ByteRange {
                offset: 0,
                length: 0,
            },
            ByteRange {
                offset: 5000,
                length: 100,
            },
        ]);
        assert_eq!(m.total(), 110);
        assert_eq!(m.locate(0), (1000, 10));
        assert_eq!(m.locate(9), (1009, 1));
        assert_eq!(m.locate(10), (5000, 100));
        assert_eq!(m.locate(109), (5099, 1));
        assert_eq!(m.locate(110), (5100, 0));
        assert_eq!(SpanMap::new(&[]).locate(3), (0, 0));
        // Pieces of 64 bytes: 1 for the first span, 2 for the second.
        assert_eq!(m.pieces(64), 3);
        assert_eq!(m.piece(64, 0), Some((0, 1000, 10)));
        assert_eq!(m.piece(64, 1), Some((10, 5000, 64)));
        assert_eq!(m.piece(64, 2), Some((74, 5064, 36)));
        assert_eq!(m.piece(64, 3), None);
    }
}
