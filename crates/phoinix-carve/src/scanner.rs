//! Chunked, parallel header search over byte ranges.
//!
//! The main thread reads one chunk at a time (sequential I/O suits slow
//! USB media); worker threads test the aligned positions of the chunk
//! against the signature set. Chunks overlap by the longest header span so
//! that a header straddling a chunk boundary is still matched.

use phoinix_block::{BlockError, BlockReader};
use phoinix_fs::ByteRange;

use crate::CarveError;
use crate::signature::SignatureSet;

/// Default chunk size.
pub const DEFAULT_CHUNK_BYTES: usize = 8 * 1024 * 1024;

/// Scan parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    /// Bytes read per chunk.
    pub chunk_bytes: usize,
    /// Only positions that are multiples of this are tested (files start
    /// on sector or cluster boundaries; 1 tests every byte).
    pub alignment: u64,
    /// Worker threads for matching (0 = available parallelism).
    pub threads: usize,
    /// Stop after this many hits.
    pub max_hits: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            alignment: 512,
            threads: 0,
            max_hits: 1_000_000,
        }
    }
}

impl ScanOptions {
    fn thread_count(&self) -> usize {
        if self.threads > 0 {
            self.threads
        } else {
            std::thread::available_parallelism().map_or(2, |n| n.get().min(16))
        }
    }
}

/// A header match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    /// Volume byte offset of the header.
    pub offset: u64,
    /// Index into the signature set.
    pub signature: usize,
}

/// The two stages of a carving run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CarveStage {
    /// Reading the eligible ranges and matching signature headers
    /// (`bytes_scanned` of `bytes_total`).
    #[default]
    Search,
    /// Going back to every hit to assemble, validate and score the file
    /// (`hits_done` of `hits`). This stage reads the source again, one hit
    /// at a time, and can take longer than the search.
    Assemble,
}

/// Progress of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanProgress {
    /// Bytes scanned so far.
    pub bytes_scanned: u64,
    /// Bytes to scan in total.
    pub bytes_total: u64,
    /// Hits found so far.
    pub hits: usize,
    /// Which stage is running.
    pub stage: CarveStage,
    /// Hits assembled (or rejected) so far; meaningful in
    /// [`CarveStage::Assemble`].
    pub hits_done: usize,
    /// Candidates produced so far by the assembly stage.
    pub candidates: usize,
    /// Bytes read from the source by the assembly stage so far.
    pub bytes_read: u64,
    /// Bytes the device could not read so far; they are treated as zeros
    /// and recorded as unreadable ranges.
    pub unreadable_bytes: u64,
}

/// Sub-read sizes tried, in order, when a read fails: a failed chunk is
/// re-read in 64 KiB blocks and a failed block in 4 KiB pieces, so that a
/// bad sector costs its own 4 KiB and not the data around it.
pub const RETRY_BLOCKS: [usize; 2] = [64 * 1024, 4096];
/// Consecutive failing pieces after which the rest of the enclosing piece is
/// written off without further attempts: every failure can cost a driver
/// timeout of many seconds.
pub const MAX_CONSECUTIVE_FAILURES: usize = 4;

/// Reads `target` at `pos`, filling it completely unless the source ends.
fn read_full(reader: &dyn BlockReader, pos: u64, target: &mut [u8]) -> Result<usize, CarveError> {
    let mut filled = 0usize;
    while filled < target.len() {
        let Some(tail) = target.get_mut(filled..) else {
            break;
        };
        let n = reader.read_at(pos.saturating_add(filled as u64), tail)?;
        if n == 0 {
            break;
        }
        filled = filled.saturating_add(n);
    }
    Ok(filled)
}

/// Records `[offset, offset + length)` as unreadable, merging with the
/// previous range when contiguous.
fn push_unreadable(unreadable: &mut Vec<ByteRange>, offset: u64, length: u64) {
    if let Some(last) = unreadable.last_mut()
        && last.end() == offset
    {
        last.length = last.length.saturating_add(length);
        return;
    }
    unreadable.push(ByteRange { offset, length });
}

/// Total bytes covered by `ranges` (assumed disjoint).
#[must_use]
pub fn unreadable_total(ranges: &[ByteRange]) -> u64 {
    ranges.iter().map(|r| r.length).sum()
}

/// Sorts `ranges` and merges the ones that overlap or touch, so that the
/// same bad region found by several reads is counted once.
pub fn merge_ranges(ranges: &mut Vec<ByteRange>) {
    ranges.sort_by_key(|r| r.offset);
    let mut merged: Vec<ByteRange> = Vec::with_capacity(ranges.len());
    for r in ranges.drain(..) {
        if let Some(last) = merged.last_mut()
            && r.offset <= last.end()
        {
            let end = last.end().max(r.end());
            last.length = end - last.offset;
        } else {
            merged.push(r);
        }
    }
    *ranges = merged;
}

/// Reads a chunk, tolerating I/O errors: a failed read is retried in the
/// pieces of [`RETRY_BLOCKS`], down to 4 KiB; what still fails is
/// zero-filled and recorded in `unreadable`, and after
/// [`MAX_CONSECUTIVE_FAILURES`] failing pieces the rest of the enclosing
/// piece is written off without further reads. Errors other than I/O errors
/// (out of bounds, permission) still propagate.
///
/// # Errors
///
/// Propagates non-I/O block errors.
pub fn read_tolerant(
    reader: &dyn BlockReader,
    pos: u64,
    target: &mut [u8],
    unreadable: &mut Vec<ByteRange>,
) -> Result<usize, CarveError> {
    read_tolerant_level(reader, pos, target, unreadable, 0)
}

fn read_tolerant_level(
    reader: &dyn BlockReader,
    pos: u64,
    target: &mut [u8],
    unreadable: &mut Vec<ByteRange>,
    level: usize,
) -> Result<usize, CarveError> {
    match read_full(reader, pos, target) {
        Ok(n) => return Ok(n),
        Err(CarveError::Block(BlockError::Io(e))) => {
            if level == 0 {
                tracing::warn!(
                    offset = pos,
                    length = target.len(),
                    error = %e,
                    "read failed; retrying in smaller blocks"
                );
            } else {
                tracing::debug!(offset = pos, length = target.len(), error = %e, "unreadable piece");
            }
        }
        Err(e) => return Err(e),
    }
    let len = target.len();
    let Some(&block) = RETRY_BLOCKS.get(level) else {
        // Smallest granularity reached: this piece is unreadable.
        target.fill(0);
        push_unreadable(unreadable, pos, len as u64);
        return Ok(len);
    };
    if len <= block {
        return read_tolerant_level(reader, pos, target, unreadable, level + 1);
    }
    let mut filled = 0usize;
    let mut failures = 0usize;
    while filled < len {
        let at = pos.saturating_add(filled as u64);
        // Pieces are aligned to multiples of the block size, so that a bad
        // sector's piece never spills over data on either side of it.
        let to_boundary = block - usize::try_from(at % block as u64).unwrap_or(0);
        let step = to_boundary.min(len.saturating_sub(filled));
        if failures >= MAX_CONSECUTIVE_FAILURES {
            if let Some(rest) = target.get_mut(filled..) {
                rest.fill(0);
            }
            push_unreadable(unreadable, at, (len - filled) as u64);
            tracing::warn!(
                offset = at,
                length = len - filled,
                "unreadable region written off without further attempts"
            );
            return Ok(len);
        }
        let Some(slice) = target.get_mut(filled..filled.saturating_add(step)) else {
            break;
        };
        let before = unreadable_total(unreadable);
        let n = read_tolerant_level(reader, at, slice, unreadable, level + 1)?;
        if n < step {
            // The source ended inside this piece.
            return Ok(filled.saturating_add(n));
        }
        if unreadable_total(unreadable).saturating_sub(before) >= step as u64 {
            failures = failures.saturating_add(1);
        } else {
            failures = 0;
        }
        filled = filled.saturating_add(step);
    }
    Ok(filled)
}

/// Tests the aligned positions of `chunk` (which starts at `base`) and
/// returns the hits in offset order.
fn match_slice(
    chunk: &[u8],
    base: u64,
    from: usize,
    to: usize,
    alignment: u64,
    set: &SignatureSet,
) -> Vec<Hit> {
    let mut hits = Vec::new();
    let alignment = alignment.max(1);
    // First aligned position at or after `from`.
    let abs_from = base.saturating_add(from as u64);
    let first_abs = abs_from.div_ceil(alignment).saturating_mul(alignment);
    let mut i = usize::try_from(first_abs.saturating_sub(base)).unwrap_or(usize::MAX);
    let step = usize::try_from(alignment).unwrap_or(usize::MAX);
    while i < to {
        if let Some(window) = chunk.get(i..) {
            for s in set.matches_at(window) {
                hits.push(Hit {
                    offset: base.saturating_add(i as u64),
                    signature: s,
                });
            }
        }
        i = i.saturating_add(step);
    }
    hits
}

/// Finds signature headers in `ranges`.
///
/// # Errors
///
/// Propagates block errors.
pub fn find_headers(
    reader: &dyn BlockReader,
    ranges: &[ByteRange],
    set: &SignatureSet,
    options: &ScanOptions,
    progress: &mut dyn FnMut(&ScanProgress),
) -> Result<Vec<Hit>, CarveError> {
    let mut unreadable = Vec::new();
    find_headers_with(
        reader,
        ranges,
        set,
        options,
        progress,
        &|| false,
        &mut unreadable,
    )
}

/// [`find_headers`] with a cancellation predicate, polled after every
/// chunk (when it returns `true` the hits found so far are returned), and
/// tolerance for unreadable regions, which are skipped and appended to
/// `unreadable` (see [`read_tolerant`]).
///
/// # Errors
///
/// Propagates block errors other than I/O errors.
#[allow(clippy::too_many_arguments)]
pub fn find_headers_with(
    reader: &dyn BlockReader,
    ranges: &[ByteRange],
    set: &SignatureSet,
    options: &ScanOptions,
    progress: &mut dyn FnMut(&ScanProgress),
    cancel: &dyn Fn() -> bool,
    unreadable: &mut Vec<ByteRange>,
) -> Result<Vec<Hit>, CarveError> {
    let overlap = set.max_header_span().saturating_sub(1);
    let chunk_bytes = options.chunk_bytes.max(overlap.saturating_add(4096));
    let threads = options.thread_count().max(1);
    let total: u64 = ranges.iter().map(|r| r.length).sum();
    let mut state = ScanProgress {
        bytes_scanned: 0,
        bytes_total: total,
        ..ScanProgress::default()
    };
    let mut hits: Vec<Hit> = Vec::new();
    let mut buf = vec![0u8; chunk_bytes.saturating_add(overlap)];
    let limit = reader.len();
    'ranges: for range in ranges {
        let end = range.end().min(limit);
        let mut pos = range.offset;
        while pos < end {
            let want = usize::try_from((end - pos).min(chunk_bytes as u64)).unwrap_or(chunk_bytes);
            // Read the chunk plus the overlap (clamped to the volume end).
            let read_len = usize::try_from((limit - pos).min(want.saturating_add(overlap) as u64))
                .unwrap_or(want);
            let Some(target) = buf.get_mut(..read_len) else {
                break;
            };
            let filled = read_tolerant(reader, pos, target, unreadable)?;
            state.unreadable_bytes = unreadable.iter().map(|r| r.length).sum();
            let Some(chunk) = buf.get(..filled) else {
                break;
            };
            let scan_to = want.min(filled);
            let mut found = if threads == 1 || scan_to < 64 * 1024 {
                match_slice(chunk, pos, 0, scan_to, options.alignment, set)
            } else {
                let per = scan_to.div_ceil(threads);
                std::thread::scope(|scope| {
                    let workers: Vec<_> = (0..threads)
                        .map(|t| {
                            let from = t.saturating_mul(per).min(scan_to);
                            let to = from.saturating_add(per).min(scan_to);
                            scope.spawn(move || {
                                match_slice(chunk, pos, from, to, options.alignment, set)
                            })
                        })
                        .collect();
                    let mut all = Vec::new();
                    for w in workers {
                        if let Ok(part) = w.join() {
                            all.extend(part);
                        }
                    }
                    all
                })
            };
            found.sort_by_key(|h| (h.offset, h.signature));
            hits.extend(found);
            state.bytes_scanned = state.bytes_scanned.saturating_add(scan_to as u64);
            state.hits = hits.len();
            progress(&state);
            if cancel() {
                tracing::info!(hits = hits.len(), "header search cancelled");
                break 'ranges;
            }
            if hits.len() >= options.max_hits {
                tracing::warn!(
                    max = options.max_hits,
                    "hit limit reached; scan stopped early"
                );
                hits.truncate(options.max_hits);
                break 'ranges;
            }
            if filled < read_len || scan_to == 0 {
                break;
            }
            pos = pos.saturating_add(scan_to as u64);
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]
    use phoinix_block::MemoryReader;

    use super::*;

    #[test]
    fn finds_aligned_headers_across_chunks_and_threads() {
        let mut data = vec![0u8; 3 * 1024 * 1024 + 700];
        let places = [
            0usize,
            512 * 3,
            1024 * 1024 - 512,
            2 * 1024 * 1024 + 4096,
            3 * 1024 * 1024,
        ];
        for p in places {
            data[p..p + 5].copy_from_slice(b"%PDF-");
        }
        // Unaligned: must not be found at 512 alignment.
        data[777..782].copy_from_slice(b"%PDF-");
        let reader = MemoryReader::new(data);
        let set = SignatureSet::builtin();
        let options = ScanOptions {
            chunk_bytes: 1024 * 1024,
            alignment: 512,
            threads: 3,
            max_hits: 100,
        };
        let mut calls = 0;
        let ranges = [ByteRange {
            offset: 0,
            length: reader.len(),
        }];
        let hits = find_headers(&reader, &ranges, &set, &options, &mut |_| calls += 1).unwrap();
        let offsets: Vec<u64> = hits.iter().map(|h| h.offset).collect();
        assert_eq!(
            offsets,
            places.iter().map(|p| *p as u64).collect::<Vec<_>>()
        );
        assert!(calls >= 3);
        let options = ScanOptions {
            alignment: 1,
            ..options
        };
        let hits = find_headers(&reader, &ranges, &set, &options, &mut |_| {}).unwrap();
        assert_eq!(hits.len(), places.len() + 1);
        // Ranges restrict the search.
        let ranges = [ByteRange {
            offset: 1024 * 1024,
            length: 1024 * 1024,
        }];
        let hits =
            find_headers(&reader, &ranges, &set, &ScanOptions::default(), &mut |_| {}).unwrap();
        assert_eq!(hits.len(), 0);
        let ranges = [ByteRange {
            offset: 2 * 1024 * 1024,
            length: 8192,
        }];
        let hits =
            find_headers(&reader, &ranges, &set, &ScanOptions::default(), &mut |_| {}).unwrap();
        assert_eq!(hits.len(), 1);
    }
}
