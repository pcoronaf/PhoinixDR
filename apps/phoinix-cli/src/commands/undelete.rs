//! Shared setup for the undelete commands: open a source, pick the volume,
//! detect its filesystem and build the matching engine.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use phoinix_core::FileSystemType;
use phoinix_device::platform_enumerator;
use phoinix_fs::DeletedFileProvider;
use phoinix_fs_exfat::{ExfatUndelete, ExfatVolume};
use phoinix_fs_fat::{FatUndelete, FatVolume};
use phoinix_fs_ntfs::{NtfsUndelete, NtfsVolume};
use phoinix_health::{DeviceKind, StorageEvidence};

use crate::source::{self, standard_probes};

/// Common source-selection arguments for scan/explain/recover.
#[derive(Debug, clap::Args)]
pub struct SourceArgs {
    /// Device path or image file.
    pub source: PathBuf,
    /// Partition index to use (default: the first partition with a
    /// supported filesystem, or the whole source when it has no partition
    /// table).
    #[arg(long, short = 'p')]
    pub partition: Option<u32>,
    /// Skip content examination (faster; lowers assessment confidence).
    #[arg(long)]
    pub no_content: bool,
}

/// An opened volume with its undelete engine.
pub struct Session {
    /// Detected filesystem.
    pub filesystem: FileSystemType,
    /// The engine.
    pub engine: Box<dyn DeletedFileProvider>,
}

impl Session {
    /// Opens the source and builds the engine for its filesystem.
    pub fn open(args: &SourceArgs) -> anyhow::Result<Self> {
        let opened = source::open(&args.source)?;
        let selected = source::select_volume(&opened, args.partition, None)?;
        let detection = standard_probes().detect(&*selected.reader);
        let filesystem = detection.filesystem();
        let storage = storage_evidence(&args.source);
        tracing::info!(partition = ?selected.partition, offset = selected.offset, %filesystem, "volume selected");
        let engine: Box<dyn DeletedFileProvider> = match filesystem {
            FileSystemType::Ntfs => {
                let volume = Arc::new(
                    NtfsVolume::open(selected.reader.clone()).context("opening NTFS volume")?,
                );
                let e = NtfsUndelete::new(volume, storage);
                Box::new(if args.no_content {
                    e.without_content_examination()
                } else {
                    e
                })
            }
            FileSystemType::Fat12 | FileSystemType::Fat16 | FileSystemType::Fat32 => {
                let volume = Arc::new(
                    FatVolume::open(selected.reader.clone()).context("opening FAT volume")?,
                );
                let e = FatUndelete::new(volume, storage);
                Box::new(if args.no_content {
                    e.without_content_examination()
                } else {
                    e
                })
            }
            FileSystemType::ExFat => {
                let volume = Arc::new(
                    ExfatVolume::open(selected.reader.clone()).context("opening exFAT volume")?,
                );
                let e = ExfatUndelete::new(volume, storage);
                Box::new(if args.no_content {
                    e.without_content_examination()
                } else {
                    e
                })
            }
            other => anyhow::bail!(
                "no undelete engine for {other}; supported: NTFS, FAT12/16/32, exFAT (use --partition to pick another volume)"
            ),
        };
        Ok(Self { filesystem, engine })
    }
}

/// Gathers what is known about the medium behind `source`.
pub fn storage_evidence(source: &Path) -> StorageEvidence {
    let enumerator = platform_enumerator();
    if !enumerator.is_device_path(source) {
        return StorageEvidence {
            device_kind: DeviceKind::Image,
            rotational: None,
            trim_supported: None,
            trim_state_known: false,
        };
    }
    let wanted = source.to_string_lossy();
    let info = enumerator.enumerate().ok().and_then(|devices| {
        devices.into_iter().find(|d| {
            d.path.as_str() == wanted || d.parent.as_ref().is_some_and(|p| p.as_str() == wanted)
        })
    });
    StorageEvidence {
        device_kind: DeviceKind::BlockDevice,
        rotational: info.and_then(|d| d.rotational),
        trim_supported: None,
        trim_state_known: false,
    }
}
