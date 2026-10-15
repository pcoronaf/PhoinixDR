//! Free space and slack of a volume, from the read-only filesystem
//! engines' allocation views (milestone M12.2).

use std::sync::Arc;

use phoinix_block::BlockReader;
use phoinix_core::{ByteRange, FileSystemType};
use phoinix_fs::{AllocationView, ProbeRegistry};
use phoinix_fs_exfat::{ExFatProbe, ExfatUndelete, ExfatVolume};
use phoinix_fs_ext::{ExtProbe, ExtUndelete, ExtVolume};
use phoinix_fs_fat::{FatProbe, FatUndelete, FatVolume};
use phoinix_fs_ntfs::{NtfsProbe, NtfsUndelete, NtfsVolume};
use phoinix_health::StorageEvidence;
use serde::{Deserialize, Serialize};

use crate::error::EraseError;

/// What a free-space target covers, for the description and the certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceDescription {
    /// Filesystem label (`NTFS`, `FAT32`, `ext`).
    pub filesystem: String,
    /// Byte offset of the volume on the path.
    pub volume_offset: u64,
    /// Length of the volume.
    pub volume_length: u64,
    /// Allocation unit in bytes.
    pub cluster_size: u64,
    /// Whether the engine has an allocation map (without one nothing is
    /// erased: every cluster would be "unknown").
    pub map_available: bool,
    /// Free bytes to overwrite.
    pub free_bytes: u64,
    /// Number of free ranges.
    pub free_ranges: usize,
    /// Whether slack is included.
    pub slack: bool,
    /// Slack bytes to overwrite (0 when not requested or unsupported).
    pub slack_bytes: u64,
    /// Number of slack ranges.
    pub slack_ranges: usize,
    /// Mount point, for the mounted (filesystem-level) mode.
    #[serde(default)]
    pub mount: Option<String>,
    /// Volume node (`\\.\E:`, `/dev/sdb1`) of a mounted volume, for the
    /// post-erase scan.
    #[serde(default)]
    pub node: Option<String>,
    /// Volume label, if known.
    #[serde(default)]
    pub label: Option<String>,
    /// Total bytes of the volume as the operating system reports them
    /// (mounted mode).
    #[serde(default)]
    pub total_bytes: Option<u64>,
}

/// The allocation view of the filesystem on `reader`, with its label.
///
/// # Errors
///
/// Returns [`EraseError::Invalid`] when no supported filesystem is found
/// or its structures cannot be read.
pub fn open_view(
    reader: Arc<dyn BlockReader>,
) -> Result<(String, Arc<dyn AllocationView>), EraseError> {
    let probes = ProbeRegistry::new()
        .with(Box::new(NtfsProbe))
        .with(Box::new(FatProbe))
        .with(Box::new(ExFatProbe))
        .with(Box::new(ExtProbe));
    let fs = probes.detect(&*reader).filesystem();
    let storage = StorageEvidence::default();
    let fail = |e: &dyn std::fmt::Display| {
        EraseError::Invalid(format!("cannot read the {fs} volume: {e}"))
    };
    let view: Arc<dyn AllocationView> = match fs {
        FileSystemType::Ntfs => {
            let v = Arc::new(NtfsVolume::open(reader).map_err(|e| fail(&e))?);
            Arc::new(NtfsUndelete::new(v, storage).without_content_examination())
        }
        FileSystemType::Fat12 | FileSystemType::Fat16 | FileSystemType::Fat32 => {
            let v = Arc::new(FatVolume::open(reader).map_err(|e| fail(&e))?);
            Arc::new(FatUndelete::new(v, storage).without_content_examination())
        }
        FileSystemType::ExFat => {
            let v = Arc::new(ExfatVolume::open(reader).map_err(|e| fail(&e))?);
            Arc::new(ExfatUndelete::new(v, storage).without_content_examination())
        }
        FileSystemType::Ext => {
            let v = Arc::new(ExtVolume::open(reader).map_err(|e| fail(&e))?);
            Arc::new(ExtUndelete::new(v, storage).without_content_examination())
        }
        other => {
            return Err(EraseError::Invalid(format!(
                "no supported filesystem on the volume (found: {other}); free space can be erased on NTFS, FAT, exFAT and ext volumes"
            )));
        }
    };
    Ok((fs.to_string(), view))
}

/// Free (and optionally slack) ranges of a volume, as absolute offsets on
/// the path the volume lies on, merged and sorted; plus the description.
///
/// # Errors
///
/// See [`open_view`]; also fails when the allocation map cannot be read.
pub fn free_space(
    reader: Arc<dyn BlockReader>,
    volume_offset: u64,
    slack: bool,
) -> Result<(SpaceDescription, Vec<ByteRange>), EraseError> {
    let volume_length = reader.len();
    let (filesystem, view) = open_view(reader)?;
    let map_available = view.map_available();
    let to_abs = |r: &phoinix_fs::ByteRange| ByteRange {
        offset: volume_offset.saturating_add(r.offset),
        length: r.length,
    };
    let free: Vec<ByteRange> = if map_available {
        view.free_ranges()
            .map_err(|e| EraseError::Invalid(format!("cannot read the allocation map: {e}")))?
            .iter()
            .filter(|r| r.end() <= volume_length)
            .map(to_abs)
            .collect()
    } else {
        Vec::new()
    };
    let slack_ranges: Vec<ByteRange> = if slack && map_available {
        view.slack_ranges()
            .map_err(|e| EraseError::Invalid(format!("cannot list the allocated files: {e}")))?
            .unwrap_or_default()
            .iter()
            .filter(|r| r.end() <= volume_length)
            .map(to_abs)
            .collect()
    } else {
        Vec::new()
    };
    let desc = SpaceDescription {
        filesystem,
        volume_offset,
        volume_length,
        cluster_size: view.cluster_size(),
        map_available,
        free_bytes: free.iter().map(|r| r.length).sum(),
        free_ranges: free.len(),
        slack,
        slack_bytes: slack_ranges.iter().map(|r| r.length).sum(),
        slack_ranges: slack_ranges.len(),
        mount: None,
        node: None,
        label: None,
        total_bytes: None,
    };
    let mut all: Vec<ByteRange> = free.into_iter().chain(slack_ranges).collect();
    all.retain(|r| r.length > 0);
    all.sort_by_key(|r| r.offset);
    let mut merged: Vec<ByteRange> = Vec::with_capacity(all.len());
    for r in all {
        match merged.last_mut() {
            Some(last) if r.offset <= last.end() => {
                let end = last.end().max(r.end());
                last.length = end - last.offset;
            }
            _ => merged.push(r),
        }
    }
    Ok((desc, merged))
}
