//! Expert Witness Format (EWF-E01, EnCase 5/6/7, FTK, SMART s01): one or
//! more segment files, each a chain of sections; the media is stored as
//! fixed-size chunks, optionally zlib-compressed, addressed by tables.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::read::ZlibDecoder;
use phoinix_block::{
    BlockError, BlockGeometry, BlockReader, BlockReaderExt, RawImage, check_request,
};
use phoinix_core::SourceId;
use phoinix_core::bytes::ByteView;
use phoinix_core::fmt::iso8601_utc;

use crate::cache::UnitCache;
use crate::error::corrupt;
use crate::info::{AcquisitionInfo, ContainerInfo, ImageFormat, StoredHashes};
use crate::{ImageError, segments};

/// EWF-E01 segment file signature.
pub const SIGNATURE_E01: [u8; 8] = *b"EVF\x09\x0d\x0a\xff\x00";
/// EWF2 (Ex01) signature: a different container that is not supported.
pub const SIGNATURE_EX01: [u8; 8] = *b"EVF2\x0d\x0a\x81\x00";
/// Logical evidence (L01) signature: not supported.
pub const SIGNATURE_L01: [u8; 8] = *b"LVF\x09\x0d\x0a\xff\x00";
/// Bytes of decoded chunks kept in memory.
const CACHE_BUDGET: usize = 16 * 1024 * 1024;
/// Largest chunk PhoinixDR accepts (sectors per chunk × bytes per sector).
const MAX_CHUNK: u64 = 16 * 1024 * 1024;
/// Largest table the reader accepts.
const MAX_TABLE_ENTRIES: u32 = 1 << 24;

const SECTION_SIZE: usize = 76;
const FILE_HEADER_SIZE: usize = 13;
const COMPRESSED: u32 = 0x8000_0000;

/// One stored chunk.
#[derive(Debug, Clone, Copy)]
struct Chunk {
    segment: u32,
    offset: u64,
    /// Bytes available in the file for this chunk (an upper bound for the
    /// last chunk of a table).
    len: u64,
    compressed: bool,
}

/// One section descriptor.
#[derive(Debug, Clone)]
struct Section {
    kind: String,
    offset: u64,
    size: u64,
    checksum_ok: bool,
}

/// An opened EWF image.
pub struct EwfImage {
    id: SourceId,
    segments: Vec<RawImage>,
    chunks: Vec<Chunk>,
    chunk_size: u64,
    length: u64,
    geometry: BlockGeometry,
    cache: UnitCache,
    info: ContainerInfo,
    checksum_failures: AtomicU64,
}

impl std::fmt::Debug for EwfImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwfImage")
            .field("segments", &self.segments.len())
            .field("chunks", &self.chunks.len())
            .field("chunk_size", &self.chunk_size)
            .field("length", &self.length)
            .finish()
    }
}

/// Volume facts from the `volume`/`disk` section.
#[derive(Debug, Clone, Default)]
struct Volume {
    chunk_count: u32,
    sectors_per_chunk: u32,
    bytes_per_sector: u32,
    sector_count: u64,
    media_type: Option<u8>,
    media_flags: Option<u8>,
    compression_level: Option<u8>,
    guid: Option<[u8; 16]>,
    smart: bool,
}

impl EwfImage {
    /// Opens the image `path` belongs to, discovering sibling segments.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError`] if a segment is missing or malformed, or the
    /// container uses an unsupported variant (EWF2 `Ex01`, logical `L01`).
    pub fn open(path: &Path) -> Result<Self, ImageError> {
        let paths = segments::ewf_segments(path);
        Self::open_segments(&paths)
    }

    /// Opens the given segment files, in order.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open).
    pub fn open_segments(paths: &[PathBuf]) -> Result<Self, ImageError> {
        let malformed = |detail: String| ImageError::Malformed {
            format: "EWF",
            detail,
        };
        let mut segments = Vec::with_capacity(paths.len());
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut volume: Option<Volume> = None;
        let mut acquisition = AcquisitionInfo::default();
        let mut have_header = false;
        let mut hashes = StoredHashes::default();
        let mut diagnostics = Vec::new();
        let mut acquisition_errors = None;
        let mut finished = false;
        for (index, p) in paths.iter().enumerate() {
            let raw = RawImage::open(p).map_err(|e| match e {
                BlockError::SourceUnavailable => ImageError::MissingSegment(p.clone()),
                other => ImageError::Block(other),
            })?;
            let head = raw.read_vec(0, FILE_HEADER_SIZE)?;
            let sig: [u8; 8] = head
                .get(..8)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| malformed("segment header too short".into()))?;
            if sig == SIGNATURE_EX01 {
                return Err(ImageError::Unsupported(
                    "EWF2 (Ex01/Lx01) containers are not supported yet".into(),
                ));
            }
            if sig == SIGNATURE_L01 {
                return Err(ImageError::Unsupported(
                    "logical evidence files (L01) hold files, not media".into(),
                ));
            }
            if sig != SIGNATURE_E01 {
                return Err(malformed(format!(
                    "{} does not start with the EWF signature",
                    p.display()
                )));
            }
            let number = ByteView::new(&head).u16_le(9).unwrap_or(0);
            if usize::from(number) != index + 1 {
                diagnostics.push(format!(
                    "{} carries segment number {number}, expected {}",
                    p.display(),
                    index + 1
                ));
            }
            let segment_index = u32::try_from(index).map_err(|_| ImageError::Overflow)?;
            let sections = read_sections(&raw)?;
            let sectors_ranges: Vec<(u64, u64)> = sections
                .iter()
                .filter(|s| s.kind == "sectors")
                .map(|s| (s.offset + SECTION_SIZE as u64, s.offset + s.size))
                .collect();
            for s in &sections {
                if !s.checksum_ok {
                    diagnostics.push(format!(
                        "section {} at {} in {} has a bad checksum",
                        s.kind,
                        s.offset,
                        p.display()
                    ));
                }
                let body_start = s.offset + SECTION_SIZE as u64;
                let body_len = s.size.saturating_sub(SECTION_SIZE as u64);
                match s.kind.as_str() {
                    "header" | "header2" if !have_header => {
                        if let Some(parsed) = read_header(&raw, body_start, body_len) {
                            acquisition = parsed;
                            have_header = true;
                        }
                    }
                    "volume" | "disk" if volume.is_none() => {
                        let body = raw.read_vec(
                            body_start,
                            usize::try_from(body_len.min(4096))
                                .map_err(|_| ImageError::Overflow)?,
                        )?;
                        volume = Some(parse_volume(&body).ok_or_else(|| {
                            malformed(format!("volume section of {} bytes", body.len()))
                        })?);
                    }
                    "table" => {
                        let Some(v) = &volume else {
                            return Err(malformed(
                                "table section before the volume section".into(),
                            ));
                        };
                        let chunk_size =
                            u64::from(v.sectors_per_chunk) * u64::from(v.bytes_per_sector);
                        let table_end = s.offset + s.size;
                        let entries = read_table(&raw, body_start, body_len)?;
                        let data_end_for = |start: u64| -> u64 {
                            sectors_ranges
                                .iter()
                                .find(|(a, b)| start >= *a && start < *b)
                                .map_or(table_end, |(_, b)| *b)
                        };
                        for (i, (start, compressed)) in entries.iter().enumerate() {
                            let end = entries
                                .get(i + 1)
                                .map_or_else(|| data_end_for(*start), |(next, _)| *next);
                            let len = end.saturating_sub(*start);
                            chunks.push(Chunk {
                                segment: segment_index,
                                offset: *start,
                                len: if *compressed {
                                    len
                                } else {
                                    len.min(chunk_size + 4)
                                },
                                compressed: *compressed,
                            });
                        }
                    }
                    "hash" => {
                        let body = raw.read_vec(body_start, 16)?;
                        if body.iter().any(|b| *b != 0) {
                            hashes.md5 = Some(hex::encode(body));
                        }
                    }
                    "digest" => {
                        let body = raw.read_vec(body_start, 36)?;
                        if body.get(..16).is_some_and(|m| m.iter().any(|b| *b != 0)) {
                            hashes.md5 = Some(hex::encode(body.get(..16).unwrap_or(&[])));
                        }
                        if body.get(16..36).is_some_and(|m| m.iter().any(|b| *b != 0)) {
                            hashes.sha1 = Some(hex::encode(body.get(16..36).unwrap_or(&[])));
                        }
                    }
                    "error2" => {
                        let body = raw.read_vec(body_start, 4)?;
                        acquisition_errors = ByteView::new(&body).u32_le(0).map(u64::from);
                    }
                    "done" => finished = true,
                    _ => {}
                }
            }
            segments.push(raw);
            if finished {
                break;
            }
        }
        let volume = volume.ok_or_else(|| malformed("no volume section".into()))?;
        if !finished {
            diagnostics.push(format!(
                "the last segment file is missing (the image ends without a done section after {} files)",
                paths.len()
            ));
        }
        let chunk_size = u64::from(volume.sectors_per_chunk) * u64::from(volume.bytes_per_sector);
        if chunk_size == 0 || chunk_size > MAX_CHUNK || volume.bytes_per_sector == 0 {
            return Err(malformed(format!(
                "{} sectors of {} bytes per chunk",
                volume.sectors_per_chunk, volume.bytes_per_sector
            )));
        }
        let length = volume
            .sector_count
            .checked_mul(u64::from(volume.bytes_per_sector))
            .ok_or(ImageError::Overflow)?;
        let needed = length.div_ceil(chunk_size);
        if (chunks.len() as u64) < needed {
            diagnostics.push(format!(
                "only {} of {needed} chunks are described by the tables; the rest reads as errors",
                chunks.len()
            ));
        }
        if u64::from(volume.chunk_count) != chunks.len() as u64 {
            diagnostics.push(format!(
                "the volume section declares {} chunks, the tables {}",
                volume.chunk_count,
                chunks.len()
            ));
        }
        let geometry = BlockGeometry::new(volume.bytes_per_sector)
            .map_err(|e| malformed(format!("bytes per sector: {e}")))?;
        let variant = if volume.smart {
            "S01 (SMART)".to_owned()
        } else {
            "E01 (EnCase/FTK)".to_owned()
        };
        let compression = match volume.compression_level {
            Some(0) if chunks.iter().all(|c| !c.compressed) => Some("none".to_owned()),
            Some(1) => Some("deflate (fast)".to_owned()),
            Some(2) => Some("deflate (best)".to_owned()),
            _ if chunks.iter().any(|c| c.compressed) => Some("deflate".to_owned()),
            _ => Some("none".to_owned()),
        };
        let media_type = volume.media_type.map(|t| {
            let base = match t {
                0 => "removable disk",
                1 => "fixed disk",
                3 => "optical disc",
                0x0E => "logical evidence",
                0x10 => "memory (RAM)",
                _ => "unknown media",
            };
            let physical = volume.media_flags.is_some_and(|f| f & 0x02 != 0);
            if physical {
                format!("{base}, physical")
            } else {
                base.to_owned()
            }
        });
        let info = ContainerInfo {
            format: ImageFormat::Ewf,
            variant,
            segments: paths.to_vec(),
            size: length,
            sector_size: volume.bytes_per_sector,
            unit_size: u32::try_from(chunk_size).ok(),
            compression,
            identifier: volume
                .guid
                .filter(|g| g.iter().any(|b| *b != 0))
                .map(format_guid),
            media_type,
            stored_hashes: hashes,
            acquisition: acquisition.any().then_some(acquisition),
            acquisition_errors,
            diagnostics,
        };
        tracing::info!(
            segments = segments.len(),
            chunks = chunks.len(),
            chunk_size,
            length,
            "EWF image opened"
        );
        Ok(Self {
            id: SourceId::new(),
            segments,
            chunks,
            chunk_size,
            length,
            geometry,
            cache: UnitCache::new(CACHE_BUDGET),
            info,
            checksum_failures: AtomicU64::new(0),
        })
    }

    /// What the container says about itself.
    #[must_use]
    pub const fn info(&self) -> &ContainerInfo {
        &self.info
    }

    /// Uncompressed chunks whose stored checksum did not match, so far.
    #[must_use]
    pub fn checksum_failures(&self) -> u64 {
        self.checksum_failures.load(Ordering::Relaxed)
    }

    /// The decoded chunk `index`.
    fn chunk(&self, index: u64) -> Result<Arc<Vec<u8>>, BlockError> {
        if let Some(c) = self.cache.get(index) {
            return Ok(c);
        }
        let chunk = self
            .chunks
            .get(usize::try_from(index).map_err(|_| BlockError::IntegerOverflow)?)
            .ok_or_else(|| corrupt(format!("EWF chunk {index} is not described by any table")))?;
        let segment = self
            .segments
            .get(chunk.segment as usize)
            .ok_or_else(|| corrupt(format!("EWF chunk {index} points at a missing segment")))?;
        let expected = usize::try_from(
            (self.length - index.saturating_mul(self.chunk_size)).min(self.chunk_size),
        )
        .map_err(|_| BlockError::IntegerOverflow)?;
        let avail = chunk.len.min(segment.len().saturating_sub(chunk.offset));
        let data = if chunk.compressed {
            let raw = segment.read_vec(
                chunk.offset,
                usize::try_from(avail.min(MAX_CHUNK + 4096))
                    .map_err(|_| BlockError::IntegerOverflow)?,
            )?;
            let mut out = Vec::with_capacity(expected);
            let mut decoder = ZlibDecoder::new(raw.as_slice()).take(expected as u64);
            decoder
                .read_to_end(&mut out)
                .map_err(|e| corrupt(format!("EWF chunk {index}: {e}")))?;
            if out.len() != expected {
                return Err(corrupt(format!(
                    "EWF chunk {index}: decompressed to {} bytes, expected {expected}",
                    out.len()
                )));
            }
            out
        } else {
            let want = (expected + 4).min(usize::try_from(avail).unwrap_or(usize::MAX));
            let mut raw = segment.read_vec(chunk.offset, want)?;
            if raw.len() >= expected + 4 {
                let stored = ByteView::new(&raw).u32_le(expected).unwrap_or(0);
                let computed = adler32(raw.get(..expected).unwrap_or(&[]));
                if stored != computed {
                    self.checksum_failures.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(index, "EWF chunk checksum mismatch");
                }
            }
            if raw.len() < expected {
                return Err(corrupt(format!(
                    "EWF chunk {index}: only {} of {expected} bytes present",
                    raw.len()
                )));
            }
            raw.truncate(expected);
            raw
        };
        let data = Arc::new(data);
        self.cache.put(index, data.clone());
        Ok(data)
    }
}

impl BlockReader for EwfImage {
    fn id(&self) -> SourceId {
        self.id
    }

    fn len(&self) -> u64 {
        self.length
    }

    fn geometry(&self) -> &BlockGeometry {
        &self.geometry
    }

    fn describe(&self) -> String {
        let first = self
            .info
            .segments
            .first()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if self.segments.len() > 1 {
            format!("{first} (+{} segments)", self.segments.len() - 1)
        } else {
            first
        }
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.length, offset, buffer.len())?;
        let mut done = 0usize;
        while done < buffer.len() {
            let pos = offset + done as u64;
            let index = pos / self.chunk_size;
            let within =
                usize::try_from(pos % self.chunk_size).map_err(|_| BlockError::IntegerOverflow)?;
            let chunk = self.chunk(index)?;
            let available = chunk.len().saturating_sub(within);
            if available == 0 {
                break;
            }
            let n = available.min(buffer.len() - done);
            let src = chunk
                .get(within..within + n)
                .ok_or(BlockError::IntegerOverflow)?;
            buffer
                .get_mut(done..done + n)
                .ok_or(BlockError::IntegerOverflow)?
                .copy_from_slice(src);
            done += n;
        }
        Ok(done)
    }
}

/// Reads the chain of section descriptors of one segment file.
fn read_sections(raw: &RawImage) -> Result<Vec<Section>, ImageError> {
    let mut out = Vec::new();
    let mut offset = FILE_HEADER_SIZE as u64;
    let len = raw.len();
    while offset + SECTION_SIZE as u64 <= len && out.len() < 1_000_000 {
        let bytes = raw.read_vec(offset, SECTION_SIZE)?;
        let v = ByteView::new(&bytes);
        let kind_raw = v.slice(0, 16).unwrap_or(&[]);
        let kind =
            String::from_utf8_lossy(kind_raw.split(|b| *b == 0).next().unwrap_or(&[])).into_owned();
        let next = v.u64_le(16).unwrap_or(0);
        let size = v.u64_le(24).unwrap_or(0);
        let stored = v.u32_le(72).unwrap_or(0);
        let checksum_ok = adler32(bytes.get(..72).unwrap_or(&[])) == stored;
        out.push(Section {
            kind: kind.clone(),
            offset,
            size,
            checksum_ok,
        });
        if kind == "done" || kind == "next" || next <= offset || next > len {
            break;
        }
        offset = next;
    }
    Ok(out)
}

/// Parses a `volume`/`disk` section body (EnCase 1052 bytes, SMART 94).
fn parse_volume(body: &[u8]) -> Option<Volume> {
    let v = ByteView::new(body);
    if body.len() >= 1052 {
        Some(Volume {
            chunk_count: v.u32_le(4)?,
            sectors_per_chunk: v.u32_le(8)?,
            bytes_per_sector: v.u32_le(12)?,
            sector_count: v.u64_le(16)?,
            media_type: v.slice(0, 1).and_then(|b| b.first().copied()),
            media_flags: v.slice(36, 1).and_then(|b| b.first().copied()),
            compression_level: v.slice(52, 1).and_then(|b| b.first().copied()),
            guid: v.slice(64, 16).and_then(|g| g.try_into().ok()),
            smart: false,
        })
    } else if body.len() >= 94 {
        Some(Volume {
            chunk_count: v.u32_le(4)?,
            sectors_per_chunk: v.u32_le(8)?,
            bytes_per_sector: v.u32_le(12)?,
            sector_count: u64::from(v.u32_le(16)?),
            media_type: None,
            media_flags: None,
            compression_level: None,
            guid: None,
            smart: true,
        })
    } else {
        None
    }
}

/// Parses a table section body into (absolute chunk offset, compressed).
fn read_table(
    raw: &RawImage,
    body_start: u64,
    body_len: u64,
) -> Result<Vec<(u64, bool)>, ImageError> {
    let head = raw.read_vec(body_start, 24)?;
    let v = ByteView::new(&head);
    let count = v.u32_le(0).unwrap_or(0);
    let base = v.u64_le(8).unwrap_or(0);
    if count > MAX_TABLE_ENTRIES || u64::from(count) * 4 + 24 > body_len {
        return Err(ImageError::Malformed {
            format: "EWF",
            detail: format!("table with {count} entries in a {body_len}-byte section"),
        });
    }
    let entries = raw.read_vec(
        body_start + 24,
        usize::try_from(u64::from(count) * 4).map_err(|_| ImageError::Overflow)?,
    )?;
    let ev = ByteView::new(&entries);
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        let e = ev.u32_le(i * 4).unwrap_or(0);
        let offset = base
            .checked_add(u64::from(e & !COMPRESSED))
            .ok_or(ImageError::Overflow)?;
        out.push((offset, e & COMPRESSED != 0));
    }
    Ok(out)
}

/// Decompresses and parses a `header`/`header2` section body.
fn read_header(raw: &RawImage, body_start: u64, body_len: u64) -> Option<AcquisitionInfo> {
    let body = raw
        .read_vec(body_start, usize::try_from(body_len.min(1 << 20)).ok()?)
        .ok()?;
    let mut text = Vec::new();
    ZlibDecoder::new(body.as_slice())
        .take(1 << 22)
        .read_to_end(&mut text)
        .ok()?;
    let decoded = if text.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = text
            .get(2..)?
            .chunks_exact(2)
            .map(|c| {
                u16::from_le_bytes([
                    c.first().copied().unwrap_or(0),
                    c.get(1).copied().unwrap_or(0),
                ])
            })
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(&text).into_owned()
    };
    parse_header_text(&decoded)
}

/// Parses the tab-separated header text: the two lines following `main`
/// are keys and values.
fn parse_header_text(text: &str) -> Option<AcquisitionInfo> {
    let mut lines = text.lines().map(|l| l.trim_end_matches('\r'));
    lines.find(|l| *l == "main")?;
    let keys: Vec<&str> = lines.next()?.split('\t').collect();
    let values: Vec<&str> = lines.next()?.split('\t').collect();
    let mut info = AcquisitionInfo::default();
    let clean = |s: &str| -> Option<String> {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_owned())
    };
    for (k, val) in keys.iter().zip(values.iter()) {
        match *k {
            "c" => info.case_number = clean(val),
            "n" => info.evidence_number = clean(val),
            "a" => info.description = clean(val),
            "e" => info.examiner = clean(val),
            "t" => info.notes = clean(val),
            "av" => info.software_version = clean(val),
            "ov" => info.operating_system = clean(val),
            "m" => info.acquisition_date = clean(val).map(|d| header_date(&d)),
            "u" => info.system_date = clean(val).map(|d| header_date(&d)),
            "md" => info.model = clean(val),
            "sn" => info.serial_number = clean(val),
            _ => {}
        }
    }
    Some(info)
}

/// Turns a header date (`2026 9 5 13 58 54` or a Unix timestamp) into
/// ISO-8601 when possible.
fn header_date(text: &str) -> String {
    let parts: Vec<&str> = text.split_whitespace().collect();
    if parts.len() == 6 {
        let nums: Option<Vec<i64>> = parts.iter().map(|p| p.parse().ok()).collect();
        if let Some(n) = nums
            && let (Some(&y), Some(&mo), Some(&d), Some(&h), Some(&mi), Some(&s)) =
                (n.first(), n.get(1), n.get(2), n.get(3), n.get(4), n.get(5))
        {
            return format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z");
        }
    }
    if let Ok(unix) = text.parse::<i64>()
        && unix > 0
    {
        return iso8601_utc(unix, 0);
    }
    text.to_owned()
}

fn format_guid(g: [u8; 16]) -> String {
    let v = ByteView::new(&g);
    format!(
        "{:08x}-{:04x}-{:04x}-{}-{}",
        v.u32_le(0).unwrap_or(0),
        v.u16_le(4).unwrap_or(0),
        v.u16_le(6).unwrap_or(0),
        hex::encode(g.get(8..10).unwrap_or(&[])),
        hex::encode(g.get(10..16).unwrap_or(&[]))
    )
}

/// Adler-32 as used by EWF section and chunk checksums.
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for block in data.chunks(5552) {
        for byte in block {
            a += u32::from(*byte);
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn adler32_matches_reference() {
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
    }

    #[test]
    fn header_text_is_parsed() {
        let text = "1\r\nmain\r\nc\tn\ta\te\tt\tav\tov\tm\tu\tp\r\nPHX-001\tEV-7\tFAT12 corpus\tExaminer Name\tacquired\t20140814\tLinux\t2026 9 5 13 58 54\t1788616749\t0\r\n\r\n";
        let info = parse_header_text(text).unwrap();
        assert_eq!(info.case_number.as_deref(), Some("PHX-001"));
        assert_eq!(info.evidence_number.as_deref(), Some("EV-7"));
        assert_eq!(info.examiner.as_deref(), Some("Examiner Name"));
        assert_eq!(
            info.acquisition_date.as_deref(),
            Some("2026-09-05T13:58:54Z")
        );
        assert_eq!(
            info.system_date.as_deref(),
            Some("2026-09-05T13:59:09.000000Z")
        );
        assert_eq!(info.operating_system.as_deref(), Some("Linux"));
    }
}
