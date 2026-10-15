//! Targets, their description and the refusal rules.

use std::path::{Path, PathBuf};

use phoinix_device::{DeviceInfo, DevicePath, platform_enumerator};
use phoinix_volume::read_partition_table;
use serde::{Deserialize, Serialize};

use crate::error::{EraseError, Refusal};
use crate::platform;

/// What to erase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EraseTarget {
    /// A whole device, partition table included.
    WholeDevice {
        /// Device path (`\\.\PhysicalDrive2`, `/dev/sdb`).
        path: PathBuf,
    },
    /// One partition of a device, by table index; the table entry stays.
    Partition {
        /// Device path.
        path: PathBuf,
        /// 1-based index in table order.
        index: u32,
    },
    /// An explicit byte range of a device.
    Range {
        /// Device path.
        path: PathBuf,
        /// First byte.
        offset: u64,
        /// Length in bytes.
        length: u64,
    },
    /// A regular file treated as a raw range (a disk image before
    /// disposal, or a test fixture).
    File {
        /// File path.
        path: PathBuf,
    },
}

impl EraseTarget {
    /// The path the target lives on.
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::WholeDevice { path }
            | Self::Partition { path, .. }
            | Self::Range { path, .. }
            | Self::File { path } => path,
        }
    }

    /// Short kind name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::WholeDevice { .. } => "device",
            Self::Partition { .. } => "partition",
            Self::Range { .. } => "range",
            Self::File { .. } => "file",
        }
    }
}

/// A mounted volume that lies on the target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountedVolume {
    /// Volume node or GUID path.
    pub node: String,
    /// Mount point or drive letter.
    pub mount: String,
    /// Whether the erase can dismount it itself (Windows) or the user must
    /// unmount it first (Linux).
    pub can_dismount: bool,
}

/// A partition of the target device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionDescription {
    /// Table index.
    pub index: u32,
    /// Byte offset.
    pub offset: u64,
    /// Length.
    pub length: u64,
    /// Type as recorded.
    pub type_name: String,
}

/// What the user will be shown before confirming, and what the engine
/// checks before writing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetDescription {
    /// The target.
    pub target: EraseTarget,
    /// The path as text.
    pub path: String,
    /// The whole disk the target lies on (the path itself for a disk).
    pub disk: Option<String>,
    /// Device information when the target is a device.
    pub device: Option<DeviceInfo>,
    /// First byte to erase.
    pub offset: u64,
    /// Bytes to erase.
    pub length: u64,
    /// Logical sector size (1 for files).
    pub sector_size: u32,
    /// The partition when the target is one.
    pub partition: Option<PartitionDescription>,
    /// Every partition of the device, for display.
    pub partitions: Vec<PartitionDescription>,
    /// Mounted volumes overlapping the target.
    pub volumes: Vec<MountedVolume>,
    /// Whether the medium is solid-state, if known.
    pub solid_state: Option<bool>,
    /// Why the target is refused; empty when it may be erased.
    pub refusals: Vec<Refusal>,
    /// What the user must type to confirm (serial number, or the path's
    /// last component when there is none).
    pub confirmation: String,
    /// A shorter accepted form (the last six characters of the serial).
    pub confirmation_short: Option<String>,
}

impl TargetDescription {
    /// Whether `text` confirms this target.
    #[must_use]
    pub fn confirms(&self, text: &str) -> bool {
        let t = text.trim();
        if t.is_empty() {
            return false;
        }
        t.eq_ignore_ascii_case(&self.confirmation)
            || self
                .confirmation_short
                .as_deref()
                .is_some_and(|s| t.eq_ignore_ascii_case(s))
    }

    /// A one-line summary (`WDC WD10 · S/N ABC · 1.0 TB · 2 volume(s)`).
    #[must_use]
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(d) = &self.device {
            parts.push(d.display_name.clone());
            if let Some(s) = &d.serial {
                parts.push(format!("S/N {s}"));
            }
        } else {
            parts.push(self.path.clone());
        }
        parts.push(phoinix_core::fmt::bytes_si(self.length));
        if let Some(p) = &self.partition {
            parts.push(format!("partition {} ({})", p.index, p.type_name));
        }
        if !self.volumes.is_empty() {
            parts.push(format!("{} mounted volume(s)", self.volumes.len()));
        }
        parts.join(" · ")
    }
}

/// Paths the erase must not touch, resolved to disks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guards {
    /// The sessions directory of the application.
    pub sessions_dir: Option<PathBuf>,
    /// Sources currently open for recovery.
    pub open_sources: Vec<PathBuf>,
    /// Allow the disk holding the executable (never the system disk).
    pub allow_executable_disk: bool,
    /// Allow the disk holding the sessions directory.
    pub allow_sessions_disk: bool,
}

fn confirmation_for(device: Option<&DeviceInfo>, path: &Path) -> (String, Option<String>) {
    if let Some(serial) = device
        .and_then(|d| d.serial.as_deref())
        .map(str::trim)
        .filter(|s| s.len() >= 4)
    {
        let short = if serial.len() > 6 {
            Some(serial.get(serial.len() - 6..).unwrap_or(serial).to_owned())
        } else {
            None
        };
        return (serial.to_owned(), short);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    (name, None)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let ca = std::fs::canonicalize(a).unwrap_or_else(|_| a.to_path_buf());
    let cb = std::fs::canonicalize(b).unwrap_or_else(|_| b.to_path_buf());
    ca == cb
}

/// Describes a target and decides whether it may be erased. Nothing is
/// written; the device is opened read-only to read its partition table.
///
/// # Errors
///
/// Returns [`EraseError`] when the path does not exist or the device
/// cannot be read. Refusals are reported inside the description, not as
/// errors, so a front-end can list them.
pub fn describe(target: &EraseTarget, guards: &Guards) -> Result<TargetDescription, EraseError> {
    let path = target.path().to_path_buf();
    let mut refusals = Vec::new();
    if guards.open_sources.iter().any(|s| same_path(s, &path)) {
        refusals.push(Refusal::OpenSource);
    }
    if let EraseTarget::File { .. } = target {
        let meta = std::fs::metadata(&path).map_err(|e| EraseError::io(&path, e))?;
        if !meta.is_file() {
            return Err(EraseError::Invalid(format!(
                "{} is not a regular file; use the device or partition target",
                path.display()
            )));
        }
        let (confirmation, confirmation_short) = confirmation_for(None, &path);
        return Ok(TargetDescription {
            target: target.clone(),
            path: path.display().to_string(),
            disk: None,
            device: None,
            offset: 0,
            length: meta.len(),
            sector_size: 1,
            partition: None,
            partitions: Vec::new(),
            volumes: Vec::new(),
            solid_state: None,
            refusals,
            confirmation,
            confirmation_short,
        });
    }

    let enumerator = platform_enumerator();
    let path_text = path.display().to_string();
    if !enumerator.is_device_path(&path) {
        return Err(EraseError::Invalid(format!(
            "{path_text} is not a block device; use the file target for image files"
        )));
    }
    let devices = enumerator.enumerate()?;
    let device = devices
        .iter()
        .find(|d| d.path.as_str() == path_text || same_path(&d.path.to_path_buf(), &path));
    let disk = device
        .and_then(|d| d.parent.as_ref().map(DevicePath::as_str).map(str::to_owned))
        .or_else(|| device.map(|d| d.path.as_str().to_owned()))
        .or_else(|| Some(path_text.clone()));
    let reader = enumerator.open_path_readonly(&DevicePath::new(path_text.clone()))?;
    let device_len = reader.len();
    let sector_size = reader.geometry().logical_sector_size;

    let table = read_partition_table(&*reader).ok();
    let partitions: Vec<PartitionDescription> = table
        .as_ref()
        .map(|t| {
            t.partitions
                .iter()
                .map(|p| PartitionDescription {
                    index: p.index,
                    offset: p.start_offset,
                    length: p.length,
                    type_name: p.partition_type.description(),
                })
                .collect()
        })
        .unwrap_or_default();

    let (offset, length, partition) = match target {
        EraseTarget::WholeDevice { .. } => (0, device_len, None),
        EraseTarget::Partition { index, .. } => match partitions.iter().find(|p| p.index == *index)
        {
            Some(p) => (p.offset, p.length, Some(p.clone())),
            None => {
                refusals.push(Refusal::NoSuchPartition { index: *index });
                (0, 0, None)
            }
        },
        EraseTarget::Range { offset, length, .. } => (*offset, *length, None),
        // Files are described above; keep the match exhaustive.
        EraseTarget::File { .. } => (0, 0, None),
    };
    if (offset.saturating_add(length) > device_len || length == 0 && partition.is_none())
        && !refusals
            .iter()
            .any(|r| matches!(r, Refusal::NoSuchPartition { .. }))
    {
        refusals.push(Refusal::OutOfBounds);
    }

    // Guards: system disk, executable disk, sessions disk.
    if let Some(disk_path) = &disk {
        if platform::system_disk().is_some_and(|s| platform::same_disk(&s, disk_path)) {
            refusals.push(Refusal::SystemDisk);
        }
        if !guards.allow_executable_disk
            && std::env::current_exe()
                .ok()
                .and_then(|e| platform::disk_of_path(&e))
                .is_some_and(|d| platform::same_disk(&d, disk_path))
        {
            refusals.push(Refusal::ExecutableDisk);
        }
        if !guards.allow_sessions_disk
            && guards
                .sessions_dir
                .as_deref()
                .and_then(platform::disk_of_path)
                .is_some_and(|d| platform::same_disk(&d, disk_path))
        {
            refusals.push(Refusal::SessionsDisk);
        }
    }

    // Mounted volumes overlapping the range.
    let range_end = offset.saturating_add(length);
    let volumes: Vec<MountedVolume> = disk
        .as_deref()
        .map(platform::volumes_on)
        .unwrap_or_default()
        .into_iter()
        .filter(|v| match (v.offset, v.length) {
            (Some(o), Some(l)) => o < range_end && o.saturating_add(l) > offset,
            _ => true,
        })
        .map(|v| MountedVolume {
            node: v.node,
            mount: v.mount,
            can_dismount: v.can_dismount,
        })
        .collect();
    for v in volumes.iter().filter(|v| !v.can_dismount) {
        refusals.push(Refusal::Mounted {
            mount: v.mount.clone(),
        });
    }

    let (confirmation, confirmation_short) = confirmation_for(device, &path);
    Ok(TargetDescription {
        target: target.clone(),
        path: path_text,
        disk,
        solid_state: device.and_then(|d| d.rotational.map(|r| !r)),
        device: device.cloned(),
        offset,
        length,
        sector_size,
        partition,
        partitions,
        volumes,
        refusals,
        confirmation,
        confirmation_short,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn file_targets_describe_and_confirm_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("stick.img");
        std::fs::write(&file, vec![7u8; 10_000]).unwrap();
        let d = describe(
            &EraseTarget::File { path: file.clone() },
            &Guards::default(),
        )
        .unwrap();
        assert_eq!(d.length, 10_000);
        assert!(d.refusals.is_empty());
        assert!(d.confirms("stick.img"));
        assert!(d.confirms(" STICK.IMG "));
        assert!(!d.confirms("other"));
        let guarded = Guards {
            open_sources: vec![file.clone()],
            ..Default::default()
        };
        let d = describe(&EraseTarget::File { path: file }, &guarded).unwrap();
        assert_eq!(d.refusals, vec![Refusal::OpenSource]);
        assert!(
            describe(
                &EraseTarget::File {
                    path: dir.path().to_path_buf()
                },
                &Guards::default()
            )
            .is_err()
        );
    }
}
