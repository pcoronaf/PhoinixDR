//! Opening sources and volumes for the service layer: partition table,
//! per-volume filesystem detection, and the engines over a selected volume.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use phoinix_block::BlockReader;
use phoinix_core::FileSystemType;
use phoinix_device::{open_source_described, platform_enumerator};
use phoinix_fs::{AllocationView, DeletedFileProvider, FsError, ProbeRegistry, WholeSource};
use phoinix_fs_exfat::{ExFatProbe, ExfatUndelete, ExfatVolume};
use phoinix_fs_ext::{ExtProbe, ExtUndelete, ExtVolume};
use phoinix_fs_fat::{FatProbe, FatUndelete, FatVolume};
use phoinix_fs_ntfs::{NtfsProbe, NtfsUndelete, NtfsVolume};
use phoinix_health::{DeviceKind, StorageEvidence};
use phoinix_image::ContainerInfo;
use phoinix_partition_recovery::{PartitionCandidate, SearchOptions, find_partitions, open_range};
use phoinix_volume::{PartitionScheme, PartitionTable, read_partition_table};

use crate::SessionError;
use crate::dto::{SourceInfo, VolumeInfo, VolumeRange};

/// Every probe PhoinixDR ships.
#[must_use]
pub fn standard_probes() -> ProbeRegistry {
    ProbeRegistry::new()
        .with(Box::new(NtfsProbe))
        .with(Box::new(FatProbe))
        .with(Box::new(ExFatProbe))
        .with(Box::new(ExtProbe))
}

/// Whether PhoinixDR has an undelete engine for `fs`.
#[must_use]
pub const fn has_engine(fs: FileSystemType) -> bool {
    matches!(
        fs,
        FileSystemType::Ntfs
            | FileSystemType::Fat12
            | FileSystemType::Fat16
            | FileSystemType::Fat32
            | FileSystemType::ExFat
            | FileSystemType::Ext
    )
}

/// Whether `path` names a block device rather than an image file.
#[must_use]
pub fn is_device_path(path: &Path) -> bool {
    platform_enumerator().is_device_path(path)
}

/// What is known about the medium behind `path`.
#[must_use]
pub fn storage_evidence(path: &Path) -> StorageEvidence {
    let enumerator = platform_enumerator();
    if !enumerator.is_device_path(path) {
        return StorageEvidence {
            device_kind: DeviceKind::Image,
            ..Default::default()
        };
    }
    let wanted = path.to_string_lossy();
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

/// An opened source with its partition table.
pub struct OpenedSource {
    /// The whole source.
    pub reader: Arc<dyn BlockReader>,
    /// Its partition table.
    pub table: PartitionTable,
    /// The image container, for image files.
    pub container: Option<ContainerInfo>,
}

/// Opens `path` and reads its partition table.
///
/// # Errors
///
/// Returns [`SessionError`] if the source cannot be opened or read.
pub fn open(path: &Path) -> Result<OpenedSource, SessionError> {
    let opened = open_source_described(path)?;
    let table = read_partition_table(&*opened.reader)?;
    Ok(OpenedSource {
        reader: opened.reader,
        table,
        container: opened.container,
    })
}

/// The container description of an image source, if it is one.
#[must_use]
pub fn container_of(path: &Path) -> Option<ContainerInfo> {
    if is_device_path(path) {
        return None;
    }
    open_source_described(path).ok().and_then(|o| o.container)
}

fn volume_info(
    partition: Option<u32>,
    offset: u64,
    length: u64,
    type_description: String,
    reader: &dyn BlockReader,
) -> VolumeInfo {
    let detection = standard_probes().detect(reader);
    let filesystem = detection.filesystem();
    VolumeInfo {
        partition,
        offset,
        length,
        type_description,
        filesystem,
        confidence: detection.best.as_ref().map_or(0, |b| b.confidence),
        supported: has_engine(filesystem),
        lost: false,
        repairs: Vec::new(),
    }
}

/// Runs the structure search over the whole source.
///
/// # Errors
///
/// Returns [`SessionError`] if the source cannot be read.
pub fn search_partitions(
    path: &Path,
    options: &SearchOptions,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<Vec<PartitionCandidate>, SessionError> {
    let opened = open(path)?;
    let mut sink = |p: &phoinix_carve::ScanProgress| progress(p.bytes_scanned, p.bytes_total);
    Ok(find_partitions(
        &opened.reader,
        Some(&opened.table),
        options,
        &mut sink,
    )?)
}

/// How to pick the volume of a source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VolumeChoice {
    /// The first volume with an engine, or the only one.
    Auto,
    /// A partition of the table.
    Partition(u32),
    /// An explicit byte range with optional repairs.
    Range(VolumeRange),
}

impl VolumeChoice {
    /// The choice a scan request expresses.
    #[must_use]
    pub fn from_request(partition: Option<u32>, volume: Option<&VolumeRange>) -> Self {
        match (volume, partition) {
            (Some(r), _) => Self::Range(r.clone()),
            (None, Some(p)) => Self::Partition(p),
            (None, None) => Self::Auto,
        }
    }

    /// The choice that reopens an already selected volume.
    #[must_use]
    pub fn reopen(info: &VolumeInfo) -> Self {
        Self::Range(VolumeRange {
            offset: info.offset,
            length: info.length,
            repairs: info.repairs.clone(),
        })
    }
}

/// Describes a source: partition table and the filesystem of every volume.
///
/// # Errors
///
/// Returns [`SessionError`] if the source cannot be opened.
pub fn inspect(path: &Path) -> Result<SourceInfo, SessionError> {
    let opened = open(path)?;
    let reader = &opened.reader;
    let mut volumes = Vec::new();
    if opened.table.scheme == PartitionScheme::None || opened.table.volumes().count() == 0 {
        volumes.push(volume_info(
            None,
            0,
            reader.len(),
            "bare volume".to_owned(),
            &**reader,
        ));
    } else {
        for part in opened.table.volumes() {
            let Ok(view) = part.open(reader.clone()) else {
                continue;
            };
            volumes.push(volume_info(
                Some(part.index),
                part.start_offset,
                part.length,
                part.partition_type.description(),
                &view,
            ));
        }
    }
    Ok(SourceInfo {
        path: path.to_path_buf(),
        is_device: is_device_path(path),
        size: reader.len(),
        sector_size: reader.geometry().logical_sector_size,
        scheme: format!("{:?}", opened.table.scheme),
        volumes,
        container: opened.container,
        diagnostics: opened
            .table
            .diagnostics
            .iter()
            .map(|d| d.to_string())
            .collect(),
    })
}

/// A selected volume with its engines.
pub struct OpenVolume {
    /// Source path as given.
    pub source_path: PathBuf,
    /// The volume.
    pub info: VolumeInfo,
    /// Reader restricted to the volume.
    pub reader: Arc<dyn BlockReader>,
    /// The metadata engine, when the filesystem has one.
    pub engine: Option<Arc<dyn DeletedFileProvider>>,
    /// Allocation knowledge (the engine's, or nothing).
    pub space: Arc<dyn AllocationView>,
    /// The medium.
    pub storage: StorageEvidence,
}

/// Opens `path`, selects a volume (`partition`, or the first with an
/// engine, or the only one) and builds its engines.
///
/// # Errors
///
/// Returns [`SessionError`] if the source or the volume cannot be opened.
pub fn open_volume(
    path: &Path,
    partition: Option<u32>,
    examine_content: bool,
) -> Result<OpenVolume, SessionError> {
    let choice = partition.map_or(VolumeChoice::Auto, VolumeChoice::Partition);
    open_volume_with(path, &choice, examine_content)
}

/// Opens `path` and the volume `choice` names, building its engines.
///
/// # Errors
///
/// Returns [`SessionError`] if the source or the volume cannot be opened.
pub fn open_volume_with(
    path: &Path,
    choice: &VolumeChoice,
    examine_content: bool,
) -> Result<OpenVolume, SessionError> {
    if let VolumeChoice::Range(range) = choice {
        let opened = open(path)?;
        let reader = open_range(
            opened.reader.clone(),
            phoinix_core::ByteRange {
                offset: range.offset,
                length: range.length,
            },
            &range.repairs,
        )?;
        let mut info = volume_info(
            None,
            range.offset,
            range.length,
            "explicit range".to_owned(),
            &*reader,
        );
        info.lost = true;
        info.repairs = range.repairs.clone();
        return build_engines(path, info, reader, examine_content);
    }
    let partition = match choice {
        VolumeChoice::Partition(p) => Some(*p),
        _ => None,
    };
    let info = inspect(path)?;
    let opened = open(path)?;
    let chosen = match partition {
        Some(index) => info
            .volumes
            .iter()
            .find(|v| v.partition == Some(index))
            .cloned()
            .ok_or_else(|| {
                SessionError::NotFound(format!(
                    "partition {index} does not exist (source has {} volume(s))",
                    info.volumes.len()
                ))
            })?,
        None => info
            .volumes
            .iter()
            .find(|v| v.supported)
            .or_else(|| info.volumes.first())
            .cloned()
            .ok_or_else(|| SessionError::NotFound("no usable volume found".into()))?,
    };
    let reader: Arc<dyn BlockReader> = match chosen.partition {
        Some(index) => {
            let part = opened
                .table
                .partitions
                .iter()
                .find(|p| p.index == index)
                .ok_or_else(|| SessionError::NotFound(format!("partition {index} vanished")))?;
            Arc::new(part.open(opened.reader.clone())?)
        }
        None => opened.reader.clone(),
    };
    build_engines(path, chosen, reader, examine_content)
}

/// Builds the engines of an opened volume.
fn build_engines(
    path: &Path,
    chosen: VolumeInfo,
    reader: Arc<dyn BlockReader>,
    examine_content: bool,
) -> Result<OpenVolume, SessionError> {
    let storage = storage_evidence(path);
    let (engine, space): (
        Option<Arc<dyn DeletedFileProvider>>,
        Arc<dyn AllocationView>,
    ) = match chosen.filesystem {
        FileSystemType::Ntfs => {
            let volume = Arc::new(NtfsVolume::open(reader.clone()).map_err(FsError::from)?);
            let e = NtfsUndelete::new(volume, storage.clone());
            let e = Arc::new(if examine_content {
                e
            } else {
                e.without_content_examination()
            });
            (Some(e.clone()), e)
        }
        FileSystemType::Fat12 | FileSystemType::Fat16 | FileSystemType::Fat32 => {
            let volume = Arc::new(FatVolume::open(reader.clone()).map_err(FsError::from)?);
            let e = FatUndelete::new(volume, storage.clone());
            let e = Arc::new(if examine_content {
                e
            } else {
                e.without_content_examination()
            });
            (Some(e.clone()), e)
        }
        FileSystemType::ExFat => {
            let volume = Arc::new(ExfatVolume::open(reader.clone()).map_err(FsError::from)?);
            let e = ExfatUndelete::new(volume, storage.clone());
            let e = Arc::new(if examine_content {
                e
            } else {
                e.without_content_examination()
            });
            (Some(e.clone()), e)
        }
        FileSystemType::Ext => {
            let volume = Arc::new(ExtVolume::open(reader.clone()).map_err(FsError::from)?);
            let e = ExtUndelete::new(volume, storage.clone());
            let e = Arc::new(if examine_content {
                e
            } else {
                e.without_content_examination()
            });
            (Some(e.clone()), e)
        }
        _ => (
            None,
            Arc::new(WholeSource::new(
                reader.len(),
                u64::from(reader.geometry().logical_sector_size.max(1)),
            )),
        ),
    };
    Ok(OpenVolume {
        source_path: path.to_path_buf(),
        info: chosen,
        reader,
        engine,
        space,
        storage,
    })
}
