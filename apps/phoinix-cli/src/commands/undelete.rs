//! Shared setup for the undelete commands: open the NTFS volume of a source
//! and build the undelete engine with storage evidence.

use std::path::{Path, PathBuf};

use anyhow::Context;
use phoinix_core::FileSystemType;
use phoinix_device::platform_enumerator;
use phoinix_fs::FileSystemObjectId;
use phoinix_fs_ntfs::{NtfsUndelete, NtfsVolume};
use phoinix_health::{DeviceKind, StorageEvidence};

use crate::source;

/// Common source-selection arguments for scan/explain/recover.
#[derive(Debug, clap::Args)]
pub struct SourceArgs {
    /// Device path or image file.
    pub source: PathBuf,
    /// Partition index to use (default: first NTFS partition, or the whole
    /// source when it has no partition table).
    #[arg(long, short = 'p')]
    pub partition: Option<u32>,
    /// Skip content examination (faster; lowers assessment confidence).
    #[arg(long)]
    pub no_content: bool,
}

/// An opened NTFS volume ready for undelete work.
pub struct Session {
    /// The NTFS volume.
    pub volume: NtfsVolume,
    /// Storage evidence for the source.
    pub storage: StorageEvidence,
}

impl Session {
    /// Opens the source and its NTFS volume.
    pub fn open(args: &SourceArgs) -> anyhow::Result<Self> {
        let opened = source::open(&args.source)?;
        let selected = source::select_volume(&opened, args.partition, Some(FileSystemType::Ntfs))?;
        let volume = NtfsVolume::open(selected.reader.clone()).context("opening NTFS volume")?;
        let storage = storage_evidence(&args.source);
        tracing::info!(partition = ?selected.partition, offset = selected.offset, "NTFS volume selected");
        Ok(Self { volume, storage })
    }

    /// Builds the undelete engine.
    pub fn undelete(&self, no_content: bool) -> NtfsUndelete<'_> {
        let engine = NtfsUndelete::new(&self.volume, self.storage.clone());
        if no_content {
            engine.without_content_examination()
        } else {
            engine
        }
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

/// Parses a candidate reference typed on the command line: `<record>` or
/// `<record>:<stream>`.
pub fn parse_reference(text: &str) -> anyhow::Result<FileSystemObjectId> {
    let (record, stream) = match text.split_once(':') {
        Some((r, s)) => (r, Some(s.to_owned())),
        None => (text, None),
    };
    let record: u64 = record.parse().with_context(|| {
        format!("invalid candidate reference {text:?}; expected an MFT record number")
    })?;
    Ok(FileSystemObjectId::Ntfs {
        record,
        sequence: 0,
        stream,
    })
}
