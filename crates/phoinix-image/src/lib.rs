//! Image containers as read-only block sources (milestone M11).
//!
//! A forensic or virtual-disk image is opened like any other source: the
//! container reader implements [`phoinix_block::BlockReader`], so every
//! partition parser, filesystem engine and carver works on it unchanged.
//! Nothing here writes; nothing here needs a native library (ADR-0013).
//!
//! | format | reader | notes |
//! |---|---|---|
//! | RAW / dd | `phoinix_block::RawImage` | one file |
//! | split RAW (`.001`, `.aa`) | [`SplitRawImage`] | siblings discovered by name |
//! | EWF-E01 (EnCase, FTK, SMART), split `E01`…`E99`, `EAA`… | [`EwfImage`] | zlib chunks, tables, stored hashes, acquisition header |
//! | VHD fixed / dynamic | [`VhdImage`] | differencing disks are refused |
//! | VHDX | [`VhdxImage`] | the log is not replayed; parents are refused |
//! | VMDK sparse / flat / stream-optimized / 2 GiB extents | [`VmdkImage`] | snapshot chains are refused |
//!
//! [`open_image`] detects the container from its content, never from its
//! extension alone, and returns the reader together with a
//! [`ContainerInfo`] describing the container, its stored hashes and the
//! acquisition metadata. [`hash::verify`] recomputes the hashes.

#![forbid(unsafe_code)]

mod cache;
mod error;
pub mod ewf;
pub mod hash;
mod info;
pub mod segments;
pub mod split;
pub mod vhd;
pub mod vhdx;
pub mod vmdk;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt, RawImage};
use phoinix_core::bytes::ByteView;

pub use error::ImageError;
pub use ewf::EwfImage;
pub use hash::{HashVerification, verify};
pub use info::{AcquisitionInfo, ContainerInfo, ImageFormat, StoredHashes};
pub use split::SplitRawImage;
pub use vhd::VhdImage;
pub use vhdx::VhdxImage;
pub use vmdk::VmdkImage;

/// A reader together with what its container says about itself.
pub struct OpenedImage {
    /// The media, as a block source.
    pub reader: Arc<dyn BlockReader>,
    /// The container description.
    pub info: ContainerInfo,
}

impl std::fmt::Debug for OpenedImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenedImage")
            .field("format", &self.info.format)
            .field("size", &self.info.size)
            .finish()
    }
}

/// Detects the container format of `path` from its content (and, for
/// split RAW, from its siblings). Returns [`ImageFormat::Raw`] for
/// anything unrecognised.
///
/// # Errors
///
/// Returns [`ImageError`] if the file cannot be read.
pub fn detect_format(path: &Path) -> Result<ImageFormat, ImageError> {
    let file = RawImage::open(path)?;
    let len = file.len();
    let head = file.read_vec(0, usize::try_from(len.min(512)).unwrap_or(0))?;
    if head.get(..8) == Some(ewf::SIGNATURE_E01.as_slice())
        || head.get(..8) == Some(ewf::SIGNATURE_EX01.as_slice())
        || head.get(..8) == Some(ewf::SIGNATURE_L01.as_slice())
    {
        return Ok(ImageFormat::Ewf);
    }
    if head.get(..8) == Some(vhdx::SIGNATURE.as_slice()) {
        return Ok(ImageFormat::Vhdx);
    }
    if ByteView::new(&head).u32_le(0) == Some(vmdk::SPARSE_MAGIC)
        || head.starts_with(vmdk::DESCRIPTOR_SIGNATURE)
    {
        return Ok(ImageFormat::Vmdk);
    }
    if len >= 512 {
        let tail = file.read_vec(len - 512, 512)?;
        if tail.get(..8) == Some(vhd::COOKIE.as_slice()) {
            return Ok(ImageFormat::Vhd);
        }
        if len >= 1024 {
            let tail = file.read_vec(len - 511, 8)?;
            if tail == vhd::COOKIE {
                return Ok(ImageFormat::Vhd);
            }
        }
    }
    if segments::split_raw_segments(path).is_some() {
        return Ok(ImageFormat::SplitRaw);
    }
    Ok(ImageFormat::Raw)
}

/// Opens `path` as an image of whatever container it is.
///
/// # Errors
///
/// Returns [`ImageError`] if the file cannot be read, a segment is
/// missing, the container is malformed or uses an unsupported feature.
pub fn open_image(path: &Path) -> Result<OpenedImage, ImageError> {
    let format = detect_format(path)?;
    tracing::info!(path = %path.display(), %format, "opening image");
    match format {
        ImageFormat::Raw => {
            let raw = RawImage::open(path)?;
            let size = raw.len();
            let sector = raw.geometry().logical_sector_size;
            Ok(OpenedImage {
                reader: Arc::new(raw),
                info: ContainerInfo::raw(path.to_path_buf(), size, sector),
            })
        }
        ImageFormat::SplitRaw => {
            let paths: Vec<PathBuf> =
                segments::split_raw_segments(path).unwrap_or_else(|| vec![path.to_path_buf()]);
            let image = SplitRawImage::open(&paths)?;
            let mut info = ContainerInfo::raw(path.to_path_buf(), image.len(), 512);
            info.format = ImageFormat::SplitRaw;
            info.variant = format!("{} files", paths.len());
            info.segments = paths;
            Ok(OpenedImage {
                reader: Arc::new(image),
                info,
            })
        }
        ImageFormat::Ewf => {
            let image = EwfImage::open(path)?;
            let info = image.info().clone();
            Ok(OpenedImage {
                reader: Arc::new(image),
                info,
            })
        }
        ImageFormat::Vhd => {
            let image = VhdImage::open(path)?;
            let info = image.info().clone();
            Ok(OpenedImage {
                reader: Arc::new(image),
                info,
            })
        }
        ImageFormat::Vhdx => {
            let image = VhdxImage::open(path)?;
            let info = image.info().clone();
            Ok(OpenedImage {
                reader: Arc::new(image),
                info,
            })
        }
        ImageFormat::Vmdk => {
            let image = VmdkImage::open(path)?;
            let info = image.info().clone();
            Ok(OpenedImage {
                reader: Arc::new(image),
                info,
            })
        }
    }
}
