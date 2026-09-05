//! What a container says about itself and about the acquisition.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Container format of an image source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageFormat {
    /// A plain RAW/dd image (one file).
    Raw,
    /// A RAW image split into numbered segment files.
    SplitRaw,
    /// Expert Witness Format (EnCase E01, FTK, SMART s01), one or more
    /// segment files.
    Ewf,
    /// Microsoft Virtual Hard Disk (fixed or dynamic).
    Vhd,
    /// Microsoft Virtual Hard Disk v2.
    Vhdx,
    /// VMware virtual disk (sparse, flat or stream-optimized extents).
    Vmdk,
}

impl ImageFormat {
    /// Human-readable label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Raw => "RAW",
            Self::SplitRaw => "split RAW",
            Self::Ewf => "EWF",
            Self::Vhd => "VHD",
            Self::Vhdx => "VHDX",
            Self::Vmdk => "VMDK",
        }
    }
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Hashes the acquisition tool stored inside the container.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct StoredHashes {
    /// MD5 of the acquired data, lower-case hex.
    pub md5: Option<String>,
    /// SHA-1 of the acquired data, lower-case hex.
    pub sha1: Option<String>,
}

impl StoredHashes {
    /// Whether any hash is stored.
    #[must_use]
    pub const fn any(&self) -> bool {
        self.md5.is_some() || self.sha1.is_some()
    }
}

/// Acquisition metadata recorded by the imaging tool (EWF header).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AcquisitionInfo {
    /// Case number.
    pub case_number: Option<String>,
    /// Evidence number.
    pub evidence_number: Option<String>,
    /// Unique description.
    pub description: Option<String>,
    /// Examiner name.
    pub examiner: Option<String>,
    /// Notes.
    pub notes: Option<String>,
    /// Acquisition date, ISO-8601 when it could be parsed, else as stored.
    pub acquisition_date: Option<String>,
    /// System date, ISO-8601 when it could be parsed, else as stored.
    pub system_date: Option<String>,
    /// Acquisition software version.
    pub software_version: Option<String>,
    /// Operating system the acquisition ran on.
    pub operating_system: Option<String>,
    /// Device model, when recorded.
    pub model: Option<String>,
    /// Device serial number, when recorded.
    pub serial_number: Option<String>,
}

impl AcquisitionInfo {
    /// Whether any field is set.
    #[must_use]
    pub fn any(&self) -> bool {
        self != &Self::default()
    }
}

/// Everything known about an image container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerInfo {
    /// Container format.
    pub format: ImageFormat,
    /// Variant within the format (`EnCase 6`, `dynamic`, `streamOptimized`).
    pub variant: String,
    /// Files making up the image, in order.
    pub segments: Vec<PathBuf>,
    /// Size of the contained media in bytes.
    pub size: u64,
    /// Logical sector size of the contained media.
    pub sector_size: u32,
    /// Compression unit (chunk, block or grain) in bytes, if any.
    pub unit_size: Option<u32>,
    /// Compression method, if any.
    pub compression: Option<String>,
    /// Container identifier (GUID) when recorded.
    pub identifier: Option<String>,
    /// Media type description when recorded.
    pub media_type: Option<String>,
    /// Hashes stored by the acquisition tool.
    pub stored_hashes: StoredHashes,
    /// Acquisition metadata, when the format records any.
    pub acquisition: Option<AcquisitionInfo>,
    /// Sectors the acquisition tool could not read, when recorded.
    pub acquisition_errors: Option<u64>,
    /// Anything worth knowing that is not an error (checksum mismatches,
    /// unflushed logs, missing redundant copies).
    pub diagnostics: Vec<String>,
}

impl ContainerInfo {
    /// A plain RAW image.
    #[must_use]
    pub fn raw(path: PathBuf, size: u64, sector_size: u32) -> Self {
        Self {
            format: ImageFormat::Raw,
            variant: "single file".into(),
            segments: vec![path],
            size,
            sector_size,
            unit_size: None,
            compression: None,
            identifier: None,
            media_type: None,
            stored_hashes: StoredHashes::default(),
            acquisition: None,
            acquisition_errors: None,
            diagnostics: Vec::new(),
        }
    }

    /// Whether the source is a container rather than a plain RAW file.
    #[must_use]
    pub const fn is_container(&self) -> bool {
        !matches!(self.format, ImageFormat::Raw)
    }
}
