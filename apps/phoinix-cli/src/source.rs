//! Composition helpers: open a source, read its partition table, pick a
//! volume, and build the standard probe registry.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use phoinix_block::BlockReader;
use phoinix_core::FileSystemType;
use phoinix_device::open_source;
use phoinix_fs::ProbeRegistry;
use phoinix_fs_exfat::ExFatProbe;
use phoinix_fs_ext::ExtProbe;
use phoinix_fs_fat::FatProbe;
use phoinix_fs_ntfs::NtfsProbe;
use phoinix_volume::{PartitionScheme, PartitionTable, read_partition_table};

/// Every probe PhoinixDR ships.
pub fn standard_probes() -> ProbeRegistry {
    ProbeRegistry::new()
        .with(Box::new(NtfsProbe))
        .with(Box::new(FatProbe))
        .with(Box::new(ExFatProbe))
        .with(Box::new(ExtProbe))
}

/// An opened source together with its partition table.
pub struct OpenedSource {
    /// The whole source.
    pub reader: Arc<dyn BlockReader>,
    /// Its partition table (possibly `None` scheme for bare volumes).
    pub table: PartitionTable,
}

/// Opens `path` and reads its partition table.
pub fn open(path: &Path) -> anyhow::Result<OpenedSource> {
    let reader = open_source(path).with_context(|| format!("opening {}", path.display()))?;
    let table = read_partition_table(&*reader).context("reading partition table")?;
    Ok(OpenedSource { reader, table })
}

/// A volume selected for filesystem work.
pub struct SelectedVolume {
    /// Reader restricted to the volume.
    pub reader: Arc<dyn BlockReader>,
    /// 1-based partition index, or `None` when the whole source is the volume.
    pub partition: Option<u32>,
    /// Byte offset of the volume inside the source.
    pub offset: u64,
}

/// Selects the volume to operate on.
///
/// With an explicit `partition` index that partition is used. Otherwise, a
/// bare volume source is used as is, and a partitioned source yields the
/// first partition whose probe matches `wanted` (or the first volume when
/// `wanted` is `None`).
pub fn select_volume(
    opened: &OpenedSource,
    partition: Option<u32>,
    wanted: Option<FileSystemType>,
) -> anyhow::Result<SelectedVolume> {
    let OpenedSource { reader, table } = opened;
    if let Some(index) = partition {
        let part = table
            .partitions
            .iter()
            .find(|p| p.index == index)
            .with_context(|| {
                format!(
                    "partition {index} does not exist (table has {} entries)",
                    table.partitions.len()
                )
            })?;
        let view = part
            .open(reader.clone())
            .with_context(|| format!("opening partition {index}"))?;
        return Ok(SelectedVolume {
            reader: Arc::new(view),
            partition: Some(index),
            offset: part.start_offset,
        });
    }
    if table.scheme == PartitionScheme::None || table.volumes().count() == 0 {
        return Ok(SelectedVolume {
            reader: reader.clone(),
            partition: None,
            offset: 0,
        });
    }
    let probes = standard_probes();
    let mut first: Option<SelectedVolume> = None;
    for part in table.volumes() {
        let Ok(view) = part.open(reader.clone()) else {
            continue;
        };
        let view: Arc<dyn BlockReader> = Arc::new(view);
        let selected = SelectedVolume {
            reader: view.clone(),
            partition: Some(part.index),
            offset: part.start_offset,
        };
        match wanted {
            None => return Ok(selected),
            Some(fs) => {
                if probes.detect(&*view).filesystem() == fs {
                    return Ok(selected);
                }
                if first.is_none() {
                    first = Some(selected);
                }
            }
        }
    }
    match wanted {
        Some(fs) => anyhow::bail!("no {fs} volume found; use --partition to choose one explicitly"),
        None => first.context("no usable partition found"),
    }
}
