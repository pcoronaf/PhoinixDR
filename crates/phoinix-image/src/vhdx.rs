//! Microsoft Virtual Hard Disk v2 (VHDX): headers with CRC-32C, a region
//! table locating the block allocation table and the metadata region,
//! and payload blocks addressed by 64-bit BAT entries.

use std::path::Path;
use std::sync::Arc;

use phoinix_block::{
    BlockError, BlockGeometry, BlockReader, BlockReaderExt, RawImage, check_request,
};
use phoinix_core::SourceId;
use phoinix_core::bytes::ByteView;
use phoinix_core::crc32c;

use crate::ImageError;
use crate::cache::UnitCache;
use crate::info::{ContainerInfo, ImageFormat, StoredHashes};

/// File type identifier.
pub const SIGNATURE: &[u8; 8] = b"vhdxfile";
const CACHE_BUDGET: usize = 32 * 1024 * 1024;
const MAX_BLOCK: u64 = 256 * 1024 * 1024;

const GUID_BAT: [u8; 16] = [
    0x66, 0x77, 0xC2, 0x2D, 0x23, 0xF6, 0x00, 0x42, 0x9D, 0x64, 0x11, 0x5E, 0x9B, 0xFD, 0x4A, 0x08,
];
const GUID_METADATA: [u8; 16] = [
    0x06, 0xA2, 0x7C, 0x8B, 0x90, 0x47, 0x9A, 0x4B, 0xB8, 0xFE, 0x57, 0x5F, 0x05, 0x0F, 0x88, 0x6E,
];
const GUID_FILE_PARAMETERS: [u8; 16] = [
    0x37, 0x67, 0xA1, 0xCA, 0x36, 0xFA, 0x43, 0x4D, 0xB3, 0xB6, 0x33, 0xF0, 0xAA, 0x44, 0xE7, 0x6B,
];
const GUID_VIRTUAL_DISK_SIZE: [u8; 16] = [
    0x24, 0x42, 0xA5, 0x2F, 0x1B, 0xCD, 0x76, 0x48, 0xB2, 0x11, 0x5D, 0xBE, 0xD8, 0x3B, 0xF4, 0xB8,
];
const GUID_LOGICAL_SECTOR_SIZE: [u8; 16] = [
    0x1D, 0xBF, 0x41, 0x81, 0x6F, 0xA9, 0x09, 0x47, 0xBA, 0x47, 0xF2, 0x33, 0xA8, 0xFA, 0xAB, 0x5F,
];
const GUID_PHYSICAL_SECTOR_SIZE: [u8; 16] = [
    0xC7, 0x48, 0xA3, 0xCD, 0x5D, 0x44, 0x71, 0x44, 0x9C, 0xC9, 0xE9, 0x88, 0x52, 0x51, 0xC5, 0x56,
];
const GUID_VIRTUAL_DISK_ID: [u8; 16] = [
    0xAB, 0x12, 0xCA, 0xBE, 0xE6, 0xB2, 0x23, 0x45, 0x93, 0xEF, 0xC3, 0x09, 0xE0, 0x00, 0xC7, 0x46,
];
const GUID_PARENT_LOCATOR: [u8; 16] = [
    0x2D, 0x5F, 0xD3, 0xA8, 0x0B, 0xB3, 0x4D, 0x45, 0xAB, 0xF7, 0xD3, 0xD8, 0x48, 0x34, 0xAB, 0x0C,
];

const STATE_FULLY_PRESENT: u64 = 6;
const STATE_PARTIALLY_PRESENT: u64 = 7;

/// An opened VHDX image.
#[derive(Debug)]
pub struct VhdxImage {
    id: SourceId,
    file: RawImage,
    length: u64,
    geometry: BlockGeometry,
    block_size: u64,
    chunk_ratio: u64,
    bat: Vec<u64>,
    cache: UnitCache,
    info: ContainerInfo,
}

impl VhdxImage {
    /// Opens a VHDX file.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError`] if the headers, region table or metadata are
    /// malformed, or the disk has a parent.
    pub fn open(path: &Path) -> Result<Self, ImageError> {
        let malformed = |detail: String| ImageError::Malformed {
            format: "VHDX",
            detail,
        };
        let file = RawImage::open(path)?;
        let ident = file.read_vec(0, 8)?;
        if ident != SIGNATURE {
            return Err(malformed("file type identifier missing".into()));
        }
        let mut diagnostics = Vec::new();
        // Headers: pick the valid one with the highest sequence number.
        let mut best: Option<(u64, Vec<u8>)> = None;
        for off in [64 * 1024u64, 128 * 1024] {
            let h = file.read_vec(off, 4096)?;
            if h.get(..4) != Some(b"head".as_slice()) {
                continue;
            }
            let stored = ByteView::new(&h).u32_le(4).unwrap_or(0);
            let mut copy = h.clone();
            if let Some(c) = copy.get_mut(4..8) {
                c.fill(0);
            }
            if crc32c::checksum(&copy) != stored {
                diagnostics.push(format!("header at {off} has a bad checksum"));
                continue;
            }
            let sequence = ByteView::new(&h).u64_le(8).unwrap_or(0);
            if best.as_ref().is_none_or(|(s, _)| sequence > *s) {
                best = Some((sequence, h));
            }
        }
        let (_, header) = best.ok_or_else(|| malformed("no valid header".into()))?;
        let log_guid = ByteView::new(&header).slice(48, 16).unwrap_or(&[]);
        if log_guid.iter().any(|b| *b != 0) {
            diagnostics.push(
                "the log holds unflushed writes that PhoinixDR does not replay; recent changes may be missing".into(),
            );
        }
        // Region table.
        let mut regions: Option<Vec<u8>> = None;
        for off in [192 * 1024u64, 256 * 1024] {
            let r = file.read_vec(off, 64 * 1024)?;
            if r.get(..4) != Some(b"regi".as_slice()) {
                continue;
            }
            let stored = ByteView::new(&r).u32_le(4).unwrap_or(0);
            let mut copy = r.clone();
            if let Some(c) = copy.get_mut(4..8) {
                c.fill(0);
            }
            if crc32c::checksum(&copy) != stored {
                diagnostics.push(format!("region table at {off} has a bad checksum"));
                continue;
            }
            regions = Some(r);
            break;
        }
        let regions = regions.ok_or_else(|| malformed("no valid region table".into()))?;
        let rv = ByteView::new(&regions);
        let count = rv.u32_le(8).unwrap_or(0).min(2047);
        let mut bat_region = None;
        let mut meta_region = None;
        for i in 0..count as usize {
            let base = 16 + i * 32;
            let guid = rv.slice(base, 16).unwrap_or(&[]);
            let offset = rv.u64_le(base + 16).unwrap_or(0);
            let length = u64::from(rv.u32_le(base + 24).unwrap_or(0));
            if guid == GUID_BAT {
                bat_region = Some((offset, length));
            } else if guid == GUID_METADATA {
                meta_region = Some((offset, length));
            }
        }
        let (bat_offset, bat_len) = bat_region.ok_or_else(|| malformed("no BAT region".into()))?;
        let (meta_offset, meta_len) =
            meta_region.ok_or_else(|| malformed("no metadata region".into()))?;
        // Metadata.
        let meta = file.read_vec(
            meta_offset,
            usize::try_from(meta_len.min(1 << 20)).map_err(|_| ImageError::Overflow)?,
        )?;
        let mv = ByteView::new(&meta);
        if meta.get(..8) != Some(b"metadata".as_slice()) {
            return Err(malformed("metadata signature missing".into()));
        }
        let entries = mv.u16_le(10).unwrap_or(0).min(2047);
        let mut block_size = 0u64;
        let mut has_parent = false;
        let mut size = 0u64;
        let mut sector_size = 512u32;
        let mut physical = None;
        let mut disk_id = None;
        for i in 0..entries as usize {
            let base = 32 + i * 32;
            let guid = mv.slice(base, 16).unwrap_or(&[]);
            let off = usize::try_from(mv.u32_le(base + 16).unwrap_or(0))
                .map_err(|_| ImageError::Overflow)?;
            let item = ByteView::new(&meta);
            if guid == GUID_FILE_PARAMETERS {
                block_size = u64::from(item.u32_le(off).unwrap_or(0));
                has_parent = item.u32_le(off + 4).unwrap_or(0) & 0x2 != 0;
            } else if guid == GUID_VIRTUAL_DISK_SIZE {
                size = item.u64_le(off).unwrap_or(0);
            } else if guid == GUID_LOGICAL_SECTOR_SIZE {
                sector_size = item.u32_le(off).unwrap_or(512);
            } else if guid == GUID_PHYSICAL_SECTOR_SIZE {
                physical = item.u32_le(off);
            } else if guid == GUID_VIRTUAL_DISK_ID {
                disk_id = item.slice(off, 16).map(hex::encode);
            } else if guid == GUID_PARENT_LOCATOR {
                has_parent = true;
            }
        }
        if has_parent {
            return Err(ImageError::Unsupported(
                "differencing VHDX disks need their parent image".into(),
            ));
        }
        if block_size == 0 || block_size > MAX_BLOCK || !block_size.is_power_of_two() {
            return Err(malformed(format!("block size {block_size}")));
        }
        if !(512..=4096).contains(&sector_size) || !sector_size.is_power_of_two() {
            return Err(malformed(format!("logical sector size {sector_size}")));
        }
        let chunk_ratio = ((1u64 << 23) * u64::from(sector_size)) / block_size;
        if chunk_ratio == 0 {
            return Err(malformed("chunk ratio of zero".into()));
        }
        let payload_blocks = size.div_ceil(block_size);
        let bat_entries = payload_blocks + payload_blocks.div_ceil(chunk_ratio);
        if bat_entries * 8 > bat_len || bat_entries > (1 << 26) {
            return Err(malformed(format!(
                "{bat_entries} BAT entries do not fit the {bat_len}-byte region"
            )));
        }
        let raw = file.read_vec(
            bat_offset,
            usize::try_from(bat_entries * 8).map_err(|_| ImageError::Overflow)?,
        )?;
        let bv = ByteView::new(&raw);
        let bat: Vec<u64> = (0..usize::try_from(bat_entries).map_err(|_| ImageError::Overflow)?)
            .map(|i| bv.u64_le(i * 8).unwrap_or(0))
            .collect();
        let mut geometry = BlockGeometry::new(sector_size).map_err(|e| malformed(e.to_string()))?;
        if let Some(p) = physical
            && let Ok(g) = geometry.clone().with_physical(p)
        {
            geometry = g;
        }
        let info = ContainerInfo {
            format: ImageFormat::Vhdx,
            variant: "dynamic".into(),
            segments: vec![path.to_path_buf()],
            size,
            sector_size,
            unit_size: u32::try_from(block_size).ok(),
            compression: None,
            identifier: disk_id,
            media_type: None,
            stored_hashes: StoredHashes::default(),
            acquisition: None,
            acquisition_errors: None,
            diagnostics,
        };
        Ok(Self {
            id: SourceId::new(),
            file,
            length: size,
            geometry,
            block_size,
            chunk_ratio,
            bat,
            cache: UnitCache::new(CACHE_BUDGET),
            info,
        })
    }

    /// What the container says about itself.
    #[must_use]
    pub const fn info(&self) -> &ContainerInfo {
        &self.info
    }

    fn block(&self, index: u64) -> Result<Arc<Vec<u8>>, BlockError> {
        if let Some(b) = self.cache.get(index) {
            return Ok(b);
        }
        let size = usize::try_from(self.block_size).map_err(|_| BlockError::IntegerOverflow)?;
        let bat_index = index + index / self.chunk_ratio;
        let entry = self
            .bat
            .get(usize::try_from(bat_index).map_err(|_| BlockError::IntegerOverflow)?)
            .copied()
            .unwrap_or(0);
        let state = entry & 0x7;
        let offset = entry & !0xF_FFFF;
        let data = match state {
            STATE_FULLY_PRESENT | STATE_PARTIALLY_PRESENT if offset != 0 => {
                let avail = self.file.len().saturating_sub(offset).min(self.block_size);
                let mut data = vec![0u8; size];
                let n = usize::try_from(avail).map_err(|_| BlockError::IntegerOverflow)?;
                if n > 0 {
                    self.file.read_exact_at(
                        offset,
                        data.get_mut(..n).ok_or(BlockError::IntegerOverflow)?,
                    )?;
                }
                data
            }
            _ => vec![0u8; size],
        };
        let data = Arc::new(data);
        self.cache.put(index, data.clone());
        Ok(data)
    }
}

impl BlockReader for VhdxImage {
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
        self.file.path().display().to_string()
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.length, offset, buffer.len())?;
        let mut done = 0usize;
        while done < buffer.len() {
            let pos = offset + done as u64;
            let index = pos / self.block_size;
            let within =
                usize::try_from(pos % self.block_size).map_err(|_| BlockError::IntegerOverflow)?;
            let block = self.block(index)?;
            let n = (block.len() - within).min(buffer.len() - done);
            buffer
                .get_mut(done..done + n)
                .ok_or(BlockError::IntegerOverflow)?
                .copy_from_slice(
                    block
                        .get(within..within + n)
                        .ok_or(BlockError::IntegerOverflow)?,
                );
            done += n;
        }
        Ok(done)
    }
}
