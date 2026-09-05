//! Chunked, parallel header search over byte ranges.
//!
//! The main thread reads one chunk at a time (sequential I/O suits slow
//! USB media); worker threads test the aligned positions of the chunk
//! against the signature set. Chunks overlap by the longest header span so
//! that a header straddling a chunk boundary is still matched.

use phoinix_block::BlockReader;
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

/// Progress of a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScanProgress {
    /// Bytes scanned so far.
    pub bytes_scanned: u64,
    /// Bytes to scan in total.
    pub bytes_total: u64,
    /// Hits found so far.
    pub hits: usize,
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
    let overlap = set.max_header_span().saturating_sub(1);
    let chunk_bytes = options.chunk_bytes.max(overlap.saturating_add(4096));
    let threads = options.thread_count().max(1);
    let total: u64 = ranges.iter().map(|r| r.length).sum();
    let mut state = ScanProgress {
        bytes_scanned: 0,
        bytes_total: total,
        hits: 0,
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
            let mut filled = 0usize;
            while filled < read_len {
                let Some(tail) = target.get_mut(filled..) else {
                    break;
                };
                let n = reader.read_at(pos.saturating_add(filled as u64), tail)?;
                if n == 0 {
                    break;
                }
                filled = filled.saturating_add(n);
            }
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
