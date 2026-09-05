//! VMware virtual disks: a text descriptor (standalone or embedded in a
//! sparse extent) lists extents; sparse extents map grains through grain
//! directories and tables, optionally deflate-compressed
//! (stream-optimized); flat extents are plain files.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use flate2::read::{DeflateDecoder, ZlibDecoder};
use phoinix_block::{
    BlockError, BlockGeometry, BlockReader, BlockReaderExt, RawImage, check_request,
};
use phoinix_core::SourceId;
use phoinix_core::bytes::ByteView;

use crate::ImageError;
use crate::cache::UnitCache;
use crate::error::corrupt;
use crate::info::{ContainerInfo, ImageFormat, StoredHashes};

/// Sparse extent magic (`KDMV`, little-endian).
pub const SPARSE_MAGIC: u32 = 0x564D_444B;
/// Descriptor file signature.
pub const DESCRIPTOR_SIGNATURE: &[u8] = b"# Disk DescriptorFile";
const CACHE_BUDGET: usize = 16 * 1024 * 1024;
const FLAG_COMPRESSED: u32 = 1 << 16;
const FLAG_MARKERS: u32 = 1 << 17;
const MAX_GRAIN: u64 = 16 * 1024 * 1024;

/// One extent of the virtual disk.
#[derive(Debug)]
enum ExtentData {
    Zero,
    Flat { file: RawImage, offset: u64 },
    Sparse(Box<SparseExtent>),
}

#[derive(Debug)]
struct Extent {
    /// First logical sector.
    start: u64,
    /// Length in sectors.
    sectors: u64,
    data: ExtentData,
}

/// A sparse extent with its grain tables loaded.
#[derive(Debug)]
struct SparseExtent {
    file: RawImage,
    grain_bytes: u64,
    compressed: bool,
    /// Grain start sectors per grain index (0 = unallocated).
    grains: Vec<u32>,
    cache: UnitCache,
}

/// An opened VMDK disk.
#[derive(Debug)]
pub struct VmdkImage {
    id: SourceId,
    path: PathBuf,
    extents: Vec<Extent>,
    length: u64,
    geometry: BlockGeometry,
    info: ContainerInfo,
}

impl VmdkImage {
    /// Opens a VMDK descriptor or monolithic sparse file.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError`] for malformed structures, missing extent
    /// files, or disks with a parent.
    pub fn open(path: &Path) -> Result<Self, ImageError> {
        let malformed = |detail: String| ImageError::Malformed {
            format: "VMDK",
            detail,
        };
        let file = RawImage::open(path)?;
        let head = file.read_vec(0, 512.min(usize::try_from(file.len()).unwrap_or(512)))?;
        let descriptor = if ByteView::new(&head).u32_le(0) == Some(SPARSE_MAGIC) {
            let h = ByteView::new(&head);
            let off = h.u64_le(28).unwrap_or(0) * 512;
            let size = h.u64_le(36).unwrap_or(0) * 512;
            if size == 0 || size > (1 << 20) {
                return Err(malformed("embedded descriptor missing".into()));
            }
            let text = file.read_vec(
                off,
                usize::try_from(size).map_err(|_| ImageError::Overflow)?,
            )?;
            String::from_utf8_lossy(&text).into_owned()
        } else if head.starts_with(DESCRIPTOR_SIGNATURE) {
            let text = file.read_vec(
                0,
                usize::try_from(file.len().min(1 << 20)).map_err(|_| ImageError::Overflow)?,
            )?;
            String::from_utf8_lossy(&text).into_owned()
        } else {
            return Err(malformed("neither a sparse extent nor a descriptor".into()));
        };
        drop(file);
        let parsed = parse_descriptor(&descriptor);
        if parsed.parent.is_some() {
            return Err(ImageError::Unsupported(
                "VMDK disks with a parent (snapshot chains) are not supported".into(),
            ));
        }
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        let mut extents = Vec::new();
        let mut segments = vec![path.to_path_buf()];
        let mut diagnostics = Vec::new();
        let mut start = 0u64;
        let mut any_compressed = false;
        let mut grain_bytes = None;
        for e in &parsed.extents {
            let extent_path = dir.join(&e.file);
            let data = match e.kind.as_str() {
                "ZERO" => ExtentData::Zero,
                "FLAT" | "VMFS" => {
                    if !extent_path.is_file() {
                        return Err(ImageError::MissingSegment(extent_path));
                    }
                    if extent_path != path {
                        segments.push(extent_path.clone());
                    }
                    ExtentData::Flat {
                        file: RawImage::open(&extent_path)?,
                        offset: e.offset * 512,
                    }
                }
                "SPARSE" | "VMFSSPARSE" => {
                    if !extent_path.is_file() {
                        return Err(ImageError::MissingSegment(extent_path));
                    }
                    if extent_path != path {
                        segments.push(extent_path.clone());
                    }
                    let sparse = SparseExtent::open(&extent_path, &mut diagnostics)?;
                    any_compressed |= sparse.compressed;
                    grain_bytes = Some(sparse.grain_bytes);
                    ExtentData::Sparse(Box::new(sparse))
                }
                other => {
                    return Err(ImageError::Unsupported(format!("VMDK extent type {other}")));
                }
            };
            extents.push(Extent {
                start,
                sectors: e.sectors,
                data,
            });
            start = start.checked_add(e.sectors).ok_or(ImageError::Overflow)?;
        }
        if extents.is_empty() {
            return Err(malformed("no extents".into()));
        }
        let length = start.checked_mul(512).ok_or(ImageError::Overflow)?;
        let info = ContainerInfo {
            format: ImageFormat::Vmdk,
            variant: parsed
                .create_type
                .clone()
                .unwrap_or_else(|| "unknown".into()),
            segments,
            size: length,
            sector_size: 512,
            unit_size: grain_bytes.and_then(|g| u32::try_from(g).ok()),
            compression: Some(if any_compressed {
                "deflate".into()
            } else {
                "none".into()
            }),
            identifier: parsed.cid.clone(),
            media_type: None,
            stored_hashes: StoredHashes::default(),
            acquisition: None,
            acquisition_errors: None,
            diagnostics,
        };
        Ok(Self {
            id: SourceId::new(),
            path: path.to_path_buf(),
            extents,
            length,
            geometry: BlockGeometry::SECTOR_512,
            info,
        })
    }

    /// What the container says about itself.
    #[must_use]
    pub const fn info(&self) -> &ContainerInfo {
        &self.info
    }
}

impl BlockReader for VmdkImage {
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
        self.path.display().to_string()
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.length, offset, buffer.len())?;
        let mut done = 0usize;
        while done < buffer.len() {
            let pos = offset + done as u64;
            let sector = pos / 512;
            let idx = self
                .extents
                .partition_point(|e| e.start <= sector)
                .saturating_sub(1);
            let extent = self.extents.get(idx).ok_or(BlockError::IntegerOverflow)?;
            let within = pos - extent.start * 512;
            let extent_bytes = extent.sectors * 512;
            let n = usize::try_from((extent_bytes - within).min((buffer.len() - done) as u64))
                .map_err(|_| BlockError::IntegerOverflow)?;
            let dst = buffer
                .get_mut(done..done + n)
                .ok_or(BlockError::IntegerOverflow)?;
            match &extent.data {
                ExtentData::Zero => dst.fill(0),
                ExtentData::Flat { file, offset: base } => {
                    file.read_exact_at(base + within, dst)?;
                }
                ExtentData::Sparse(s) => s.read(within, dst)?,
            }
            done += n;
        }
        Ok(done)
    }
}

impl SparseExtent {
    fn open(path: &Path, diagnostics: &mut Vec<String>) -> Result<Self, ImageError> {
        let malformed = |detail: String| ImageError::Malformed {
            format: "VMDK",
            detail,
        };
        let file = RawImage::open(path)?;
        let mut header = file.read_vec(0, 512)?;
        let mut h = ByteView::new(&header);
        if h.u32_le(0) != Some(SPARSE_MAGIC) {
            return Err(malformed(format!(
                "{} is not a sparse extent",
                path.display()
            )));
        }
        let flags = h.u32_le(8).unwrap_or(0);
        let capacity = h.u64_le(12).unwrap_or(0);
        let grain_sectors = h.u64_le(20).unwrap_or(0);
        let gtes_per_gt = u64::from(h.u32_le(44).unwrap_or(0));
        let mut gd_offset = h.u64_le(56).unwrap_or(0);
        let compressed = flags & FLAG_COMPRESSED != 0;
        if compressed && h.u16_le(77) != Some(1) {
            return Err(ImageError::Unsupported(format!(
                "VMDK compression algorithm {}",
                h.u16_le(77).unwrap_or(0)
            )));
        }
        if gd_offset == u64::MAX {
            // Stream-optimized: the footer (a header copy) precedes the
            // end-of-stream marker and carries the real directory offset.
            let footer_at = file.len().checked_sub(1024).ok_or(ImageError::Overflow)?;
            header = file.read_vec(footer_at, 512)?;
            h = ByteView::new(&header);
            if h.u32_le(0) != Some(SPARSE_MAGIC) {
                return Err(malformed("stream-optimized footer missing".into()));
            }
            gd_offset = h.u64_le(56).unwrap_or(0);
        }
        if grain_sectors == 0 || gtes_per_gt == 0 || grain_sectors * 512 > MAX_GRAIN {
            return Err(malformed(format!(
                "grain of {grain_sectors} sectors, {gtes_per_gt} entries per table"
            )));
        }
        if h.slice(72, 1).and_then(|b| b.first().copied()) == Some(1) {
            diagnostics.push(format!("{} was not cleanly closed", path.display()));
        }
        let grain_count = capacity.div_ceil(grain_sectors);
        let gt_count = grain_count.div_ceil(gtes_per_gt);
        if gt_count > (1 << 22) {
            return Err(malformed(format!("{gt_count} grain tables")));
        }
        let gd = file.read_vec(
            gd_offset * 512,
            usize::try_from(gt_count * 4).map_err(|_| ImageError::Overflow)?,
        )?;
        let gdv = ByteView::new(&gd);
        let mut grains = Vec::with_capacity(usize::try_from(grain_count).unwrap_or(0));
        for t in 0..usize::try_from(gt_count).map_err(|_| ImageError::Overflow)? {
            let gt_sector = u64::from(gdv.u32_le(t * 4).unwrap_or(0));
            let entries_here = gtes_per_gt.min(grain_count - grains.len() as u64);
            if gt_sector == 0 {
                grains.extend(std::iter::repeat_n(
                    0u32,
                    usize::try_from(entries_here).unwrap_or(0),
                ));
                continue;
            }
            let gt = file.read_vec(
                gt_sector * 512,
                usize::try_from(entries_here * 4).map_err(|_| ImageError::Overflow)?,
            )?;
            let gtv = ByteView::new(&gt);
            for i in 0..usize::try_from(entries_here).map_err(|_| ImageError::Overflow)? {
                grains.push(gtv.u32_le(i * 4).unwrap_or(0));
            }
        }
        let _ = flags & FLAG_MARKERS;
        Ok(Self {
            file,
            grain_bytes: grain_sectors * 512,
            compressed,
            grains,
            cache: UnitCache::new(CACHE_BUDGET),
        })
    }

    fn grain(&self, index: u64) -> Result<Arc<Vec<u8>>, BlockError> {
        if let Some(g) = self.cache.get(index) {
            return Ok(g);
        }
        let size = usize::try_from(self.grain_bytes).map_err(|_| BlockError::IntegerOverflow)?;
        let sector = self
            .grains
            .get(usize::try_from(index).map_err(|_| BlockError::IntegerOverflow)?)
            .copied()
            .unwrap_or(0);
        let data = if sector <= 1 {
            vec![0u8; size]
        } else if self.compressed {
            let at = u64::from(sector) * 512;
            let marker = self.file.read_vec(at, 12)?;
            let compressed_len = ByteView::new(&marker).u32_le(8).unwrap_or(0);
            if compressed_len == 0 || u64::from(compressed_len) > MAX_GRAIN {
                return Err(corrupt(format!("VMDK grain {index}: bad marker")));
            }
            let raw = self.file.read_vec(at + 12, compressed_len as usize)?;
            let mut out = Vec::with_capacity(size);
            let zlib = ZlibDecoder::new(raw.as_slice())
                .take(self.grain_bytes)
                .read_to_end(&mut out);
            if zlib.is_err() || out.is_empty() {
                out.clear();
                DeflateDecoder::new(raw.as_slice())
                    .take(self.grain_bytes)
                    .read_to_end(&mut out)
                    .map_err(|e| corrupt(format!("VMDK grain {index}: {e}")))?;
            }
            out.resize(size, 0);
            out
        } else {
            let at = u64::from(sector) * 512;
            let avail = self.file.len().saturating_sub(at).min(self.grain_bytes);
            let mut out = vec![0u8; size];
            let n = usize::try_from(avail).map_err(|_| BlockError::IntegerOverflow)?;
            if n > 0 {
                self.file
                    .read_exact_at(at, out.get_mut(..n).ok_or(BlockError::IntegerOverflow)?)?;
            }
            out
        };
        let data = Arc::new(data);
        self.cache.put(index, data.clone());
        Ok(data)
    }

    fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<(), BlockError> {
        let mut done = 0usize;
        while done < buffer.len() {
            let pos = offset + done as u64;
            let index = pos / self.grain_bytes;
            let within =
                usize::try_from(pos % self.grain_bytes).map_err(|_| BlockError::IntegerOverflow)?;
            let grain = self.grain(index)?;
            let n = (grain.len() - within).min(buffer.len() - done);
            buffer
                .get_mut(done..done + n)
                .ok_or(BlockError::IntegerOverflow)?
                .copy_from_slice(
                    grain
                        .get(within..within + n)
                        .ok_or(BlockError::IntegerOverflow)?,
                );
            done += n;
        }
        Ok(())
    }
}

/// One extent line of a descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtentLine {
    kind: String,
    sectors: u64,
    file: String,
    offset: u64,
}

#[derive(Debug, Default)]
struct Descriptor {
    create_type: Option<String>,
    cid: Option<String>,
    parent: Option<String>,
    extents: Vec<ExtentLine>,
}

/// Parses the descriptor text.
fn parse_descriptor(text: &str) -> Descriptor {
    let mut d = Descriptor::default();
    for line in text.lines() {
        let line = line.trim().trim_matches(char::from(0));
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().trim_matches('"').to_owned();
            match k.trim() {
                "createType" => d.create_type = Some(v),
                "CID" => d.cid = Some(v),
                "parentFileNameHint" => d.parent = Some(v),
                _ => {}
            }
            continue;
        }
        let mut parts = line.splitn(4, ' ');
        let access = parts.next().unwrap_or("");
        if !matches!(access, "RW" | "RDONLY" | "NOACCESS") {
            continue;
        }
        let Some(sectors) = parts.next().and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        let kind = parts.next().unwrap_or("").to_owned();
        let rest = parts.next().unwrap_or("");
        let (file, offset) = if let Some(after) = rest.strip_prefix('"') {
            let end = after.find('"').unwrap_or(after.len());
            let file = after.get(..end).unwrap_or("").to_owned();
            let offset = after
                .get(end + 1..)
                .and_then(|o| o.trim().parse::<u64>().ok())
                .unwrap_or(0);
            (file, offset)
        } else {
            (rest.trim().to_owned(), 0)
        };
        d.extents.push(ExtentLine {
            kind,
            sectors,
            file,
            offset,
        });
    }
    d
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn descriptor_lines() {
        let text = "# Disk DescriptorFile\nversion=1\nCID=698642ef\ncreateType=\"twoGbMaxExtentSparse\"\nRW 4096 SPARSE \"a-s001.vmdk\"\nRW 4096 FLAT \"a-f002.vmdk\" 128\nRW 100 ZERO\n";
        let d = parse_descriptor(text);
        assert_eq!(d.create_type.as_deref(), Some("twoGbMaxExtentSparse"));
        assert_eq!(d.extents.len(), 3);
        assert_eq!(d.extents[1].offset, 128);
        assert_eq!(d.extents[1].file, "a-f002.vmdk");
        assert_eq!(d.extents[2].kind, "ZERO");
        assert!(d.parent.is_none());
    }
}
