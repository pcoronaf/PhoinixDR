//! Microsoft Virtual Hard Disk (VHD): a footer at the end of the file;
//! fixed images store the data verbatim, dynamic images map 2 MiB blocks
//! through a block allocation table, each block preceded by a sector
//! bitmap.

use std::path::Path;
use std::sync::Arc;

use phoinix_block::{
    BlockError, BlockGeometry, BlockReader, BlockReaderExt, RawImage, check_request,
};
use phoinix_core::SourceId;
use phoinix_core::bytes::ByteView;

use crate::ImageError;
use crate::cache::UnitCache;
use crate::error::corrupt;
use crate::info::{ContainerInfo, ImageFormat, StoredHashes};

/// Footer cookie.
pub const COOKIE: &[u8; 8] = b"conectix";
const DYNAMIC_COOKIE: &[u8; 8] = b"cxsparse";
const UNALLOCATED: u32 = 0xFFFF_FFFF;
const CACHE_BUDGET: usize = 16 * 1024 * 1024;
const MAX_BLOCK: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiskType {
    Fixed,
    Dynamic,
    Differencing,
}

/// Dynamic-disk mapping.
#[derive(Debug)]
struct Dynamic {
    block_size: u64,
    /// Bytes of sector bitmap preceding each block's data.
    bitmap_bytes: u64,
    bat: Vec<u32>,
}

/// An opened VHD image.
#[derive(Debug)]
pub struct VhdImage {
    id: SourceId,
    file: RawImage,
    length: u64,
    geometry: BlockGeometry,
    dynamic: Option<Dynamic>,
    cache: UnitCache,
    info: ContainerInfo,
}

impl VhdImage {
    /// Opens a VHD file.
    ///
    /// # Errors
    ///
    /// Returns [`ImageError`] if the footer or dynamic header is malformed,
    /// or the disk is a differencing disk (needs its parent).
    pub fn open(path: &Path) -> Result<Self, ImageError> {
        let malformed = |detail: String| ImageError::Malformed {
            format: "VHD",
            detail,
        };
        let file = RawImage::open(path)?;
        let file_len = file.len();
        if file_len < 512 {
            return Err(malformed("file shorter than a footer".into()));
        }
        // The footer is the last 512 bytes (511 in very old images).
        let mut footer = file.read_vec(file_len - 512, 512)?;
        let mut diagnostics = Vec::new();
        if footer.get(..8) != Some(COOKIE.as_slice()) {
            let alt = file.read_vec(file_len - 511, 511)?;
            if alt.get(..8) == Some(COOKIE.as_slice()) {
                footer = alt;
                footer.push(0);
                diagnostics.push("511-byte footer (legacy layout)".into());
            } else {
                let head = file.read_vec(0, 512)?;
                if head.get(..8) == Some(COOKIE.as_slice()) {
                    footer = head;
                    diagnostics.push(
                        "the footer at the end is damaged; the copy at the start was used".into(),
                    );
                } else {
                    return Err(malformed("no footer cookie".into()));
                }
            }
        }
        let v = ByteView::new(&footer);
        let data_offset = v.u64_be(16).unwrap_or(u64::MAX);
        let current_size = v.u64_be(48).unwrap_or(0);
        let disk_type = match v.u32_be(60).unwrap_or(0) {
            2 => DiskType::Fixed,
            3 => DiskType::Dynamic,
            4 => DiskType::Differencing,
            other => return Err(ImageError::Unsupported(format!("VHD disk type {other}"))),
        };
        let stored_checksum = v.u32_be(64).unwrap_or(0);
        let computed = footer
            .iter()
            .enumerate()
            .filter(|(i, _)| !(64..68).contains(i))
            .fold(0u32, |acc, (_, b)| acc.wrapping_add(u32::from(*b)));
        if !computed != stored_checksum {
            diagnostics.push("footer checksum mismatch".into());
        }
        let uuid = v.slice(68, 16).map(|g| {
            format!(
                "{}-{}-{}-{}-{}",
                hex::encode(g.get(0..4).unwrap_or(&[])),
                hex::encode(g.get(4..6).unwrap_or(&[])),
                hex::encode(g.get(6..8).unwrap_or(&[])),
                hex::encode(g.get(8..10).unwrap_or(&[])),
                hex::encode(g.get(10..16).unwrap_or(&[]))
            )
        });
        let creator = String::from_utf8_lossy(v.slice(28, 4).unwrap_or(&[]))
            .trim_matches(char::from(0))
            .to_owned();
        if disk_type == DiskType::Differencing {
            return Err(ImageError::Unsupported(
                "differencing VHD disks need their parent image; open the parent chain with a hypervisor tool first".into(),
            ));
        }
        let mut dynamic = None;
        if disk_type == DiskType::Dynamic {
            let header = file.read_vec(data_offset, 1024)?;
            let h = ByteView::new(&header);
            if header.get(..8) != Some(DYNAMIC_COOKIE.as_slice()) {
                return Err(malformed("dynamic header cookie missing".into()));
            }
            let table_offset = h.u64_be(16).unwrap_or(0);
            let max_entries = h.u32_be(28).unwrap_or(0);
            let block_size = u64::from(h.u32_be(32).unwrap_or(0));
            if block_size == 0 || block_size > MAX_BLOCK || block_size % 512 != 0 {
                return Err(malformed(format!("block size {block_size}")));
            }
            if u64::from(max_entries) > (1 << 24) {
                return Err(malformed(format!("{max_entries} BAT entries")));
            }
            let raw = file.read_vec(
                table_offset,
                usize::try_from(u64::from(max_entries) * 4).map_err(|_| ImageError::Overflow)?,
            )?;
            let rv = ByteView::new(&raw);
            let bat: Vec<u32> = (0..max_entries as usize)
                .map(|i| rv.u32_be(i * 4).unwrap_or(UNALLOCATED))
                .collect();
            let sectors_per_block = block_size / 512;
            let bitmap_bytes = sectors_per_block.div_ceil(8).div_ceil(512) * 512;
            dynamic = Some(Dynamic {
                block_size,
                bitmap_bytes,
                bat,
            });
        } else if current_size > file_len {
            diagnostics.push(format!(
                "the footer declares {current_size} bytes but the file holds {file_len}; reads beyond the file fail"
            ));
        }
        let info = ContainerInfo {
            format: ImageFormat::Vhd,
            variant: match disk_type {
                DiskType::Fixed => "fixed".into(),
                DiskType::Dynamic => "dynamic".into(),
                DiskType::Differencing => "differencing".into(),
            },
            segments: vec![path.to_path_buf()],
            size: current_size,
            sector_size: 512,
            unit_size: dynamic
                .as_ref()
                .and_then(|d| u32::try_from(d.block_size).ok()),
            compression: None,
            identifier: uuid,
            media_type: (!creator.is_empty()).then(|| format!("created by {creator}")),
            stored_hashes: StoredHashes::default(),
            acquisition: None,
            acquisition_errors: None,
            diagnostics,
        };
        Ok(Self {
            id: SourceId::new(),
            file,
            length: current_size,
            geometry: BlockGeometry::SECTOR_512,
            dynamic,
            cache: UnitCache::new(CACHE_BUDGET),
            info,
        })
    }

    /// What the container says about itself.
    #[must_use]
    pub const fn info(&self) -> &ContainerInfo {
        &self.info
    }

    /// The decoded dynamic block `index` (zeros when unallocated), honouring
    /// the sector bitmap.
    fn block(&self, d: &Dynamic, index: u64) -> Result<Arc<Vec<u8>>, BlockError> {
        if let Some(b) = self.cache.get(index) {
            return Ok(b);
        }
        let size = usize::try_from(d.block_size).map_err(|_| BlockError::IntegerOverflow)?;
        let entry = d
            .bat
            .get(usize::try_from(index).map_err(|_| BlockError::IntegerOverflow)?)
            .copied()
            .unwrap_or(UNALLOCATED);
        let data = if entry == UNALLOCATED {
            vec![0u8; size]
        } else {
            let start = u64::from(entry) * 512;
            let bitmap = self.file.read_vec(
                start,
                usize::try_from(d.bitmap_bytes).map_err(|_| BlockError::IntegerOverflow)?,
            )?;
            let data_start = start
                .checked_add(d.bitmap_bytes)
                .ok_or(BlockError::IntegerOverflow)?;
            let avail = self.file.len().saturating_sub(data_start).min(d.block_size);
            let mut data = vec![0u8; size];
            let n = usize::try_from(avail).map_err(|_| BlockError::IntegerOverflow)?;
            if n > 0 {
                self.file.read_exact_at(
                    data_start,
                    data.get_mut(..n).ok_or(BlockError::IntegerOverflow)?,
                )?;
            }
            // Sectors whose bitmap bit is clear were never written.
            for (sector, chunk) in data.chunks_mut(512).enumerate() {
                let bit = bitmap
                    .get(sector / 8)
                    .is_some_and(|b| b & (0x80 >> (sector % 8)) != 0);
                if !bit {
                    chunk.fill(0);
                }
            }
            data
        };
        let data = Arc::new(data);
        self.cache.put(index, data.clone());
        Ok(data)
    }
}

impl BlockReader for VhdImage {
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
        let Some(d) = &self.dynamic else {
            // Fixed: data precedes the footer.
            if offset.saturating_add(buffer.len() as u64) > self.file.len().saturating_sub(512) {
                return Err(corrupt("VHD fixed image is truncated"));
            }
            return self.file.read_at(offset, buffer);
        };
        let mut done = 0usize;
        while done < buffer.len() {
            let pos = offset + done as u64;
            let index = pos / d.block_size;
            let within =
                usize::try_from(pos % d.block_size).map_err(|_| BlockError::IntegerOverflow)?;
            let block = self.block(d, index)?;
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
