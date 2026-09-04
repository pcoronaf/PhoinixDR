//! The NTFS volume facade.

use std::sync::Arc;

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::bytes::{ByteView, utf16le_to_string_lossy};
use serde::{Deserialize, Serialize};

use crate::NtfsError;
use crate::attribute::AttributeType;
use crate::boot::NtfsBootSector;
use crate::data::DataStorage;
use crate::file::NtfsFile;
use crate::mft::{Mft, VOLUME_RECORD};
use crate::stream::NtfsDataStream;
use crate::tree::PathResolver;

/// Volume-level information from `$Volume`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct VolumeInformation {
    /// Volume label.
    pub name: Option<String>,
    /// NTFS version (major, minor).
    pub version: Option<(u8, u8)>,
    /// `$VOLUME_INFORMATION` flags (bit 0: dirty).
    pub flags: Option<u16>,
}

/// An opened NTFS volume.
pub struct NtfsVolume {
    reader: Arc<dyn BlockReader>,
    boot: NtfsBootSector,
    mft: Mft,
}

impl std::fmt::Debug for NtfsVolume {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NtfsVolume")
            .field("source", &self.reader.describe())
            .field("boot", &self.boot)
            .finish()
    }
}

impl NtfsVolume {
    /// Opens the volume whose boot sector is at offset 0 of `reader`.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::InvalidBootSector`] or a bootstrap error.
    pub fn open(reader: Arc<dyn BlockReader>) -> Result<Self, NtfsError> {
        let sector = reader.read_vec(0, 512)?;
        let boot = NtfsBootSector::parse(&sector)?;
        if !boot.fits_in(reader.len()) {
            tracing::warn!(
                declared = boot.volume_bytes().unwrap_or(0),
                available = reader.len(),
                "NTFS volume declares more sectors than the source holds; treating as truncated"
            );
        }
        let mft = Mft::bootstrap(reader.clone(), &boot)?;
        Ok(Self { reader, boot, mft })
    }

    /// The boot sector.
    #[must_use]
    pub const fn boot(&self) -> &NtfsBootSector {
        &self.boot
    }

    /// The MFT.
    #[must_use]
    pub const fn mft(&self) -> &Mft {
        &self.mft
    }

    /// The underlying reader.
    #[must_use]
    pub fn reader(&self) -> &Arc<dyn BlockReader> {
        &self.reader
    }

    /// Cluster size in bytes.
    #[must_use]
    pub const fn cluster_size(&self) -> u32 {
        self.boot.cluster_size
    }

    /// Number of clusters in the volume.
    #[must_use]
    pub const fn total_clusters(&self) -> u64 {
        self.boot.total_clusters()
    }

    /// Reads and assembles the file at `record`.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError`] if the base record is unusable.
    pub fn file(&self, record: u64) -> Result<NtfsFile, NtfsError> {
        let base = self.mft.record(record)?;
        NtfsFile::assemble(
            &self.mft,
            &base,
            self.boot.cluster_size,
            self.boot.total_clusters(),
        )
    }

    /// Iterates every base record as a file. Extension records and corrupt
    /// records are reported as errors; iteration always continues.
    pub fn files(&self) -> impl Iterator<Item = (u64, Result<NtfsFile, NtfsError>)> + '_ {
        self.mft.records().map(move |(n, rec)| {
            let result = rec.and_then(|r| {
                if !r.header().is_base() {
                    return Err(NtfsError::InvalidRecord {
                        record: n,
                        reason: "extension record".into(),
                    });
                }
                NtfsFile::assemble(
                    &self.mft,
                    &r,
                    self.boot.cluster_size,
                    self.boot.total_clusters(),
                )
            });
            (n, result)
        })
    }

    /// Opens a data stream of `file` (`None` = unnamed stream).
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError::NotFound`] if the stream does not exist and
    /// [`NtfsError::Unsupported`] for compressed or encrypted streams.
    pub fn open_stream(
        &self,
        file: &NtfsFile,
        name: Option<&str>,
    ) -> Result<NtfsDataStream, NtfsError> {
        let descriptor = file.stream(name).ok_or_else(|| {
            NtfsError::NotFound(format!(
                "stream {:?} of record {}",
                name.unwrap_or(""),
                file.reference.record
            ))
        })?;
        match &descriptor.storage {
            DataStorage::Resident { value } => Ok(NtfsDataStream::resident(value.clone())),
            DataStorage::NonResident {
                runs,
                real_size,
                initialized_size,
                ..
            } => Ok(NtfsDataStream::non_resident(
                self.reader.clone(),
                self.boot.cluster_size,
                runs.clone(),
                *real_size,
                *initialized_size,
            )),
            DataStorage::UnsupportedCompressed { .. } => {
                Err(NtfsError::Unsupported("NTFS-compressed stream".into()))
            }
            DataStorage::UnsupportedEncrypted { .. } => {
                Err(NtfsError::Unsupported("EFS-encrypted stream".into()))
            }
        }
    }

    /// A path resolver over this volume.
    #[must_use]
    pub fn resolver(&self) -> PathResolver<'_> {
        PathResolver::new(self)
    }

    /// Reads label and version from `$Volume`.
    ///
    /// # Errors
    ///
    /// Returns [`NtfsError`] if record 3 cannot be read.
    pub fn volume_information(&self) -> Result<VolumeInformation, NtfsError> {
        let record = self.mft.record(VOLUME_RECORD)?;
        let mut info = VolumeInformation::default();
        for attr in record.attributes().flatten() {
            match attr.header.attribute_type {
                AttributeType::VolumeName => {
                    if let Some(v) = attr.resident_value() {
                        let name = utf16le_to_string_lossy(v);
                        info.name = if name.is_empty() { None } else { Some(name) };
                    }
                }
                AttributeType::VolumeInformation => {
                    if let Some(v) = attr.resident_value() {
                        let view = ByteView::new(v);
                        if let (Some(major), Some(minor)) = (view.u8(8), view.u8(9)) {
                            info.version = Some((major, minor));
                        }
                        info.flags = view.u16_le(10);
                    }
                }
                _ => {}
            }
        }
        Ok(info)
    }
}
