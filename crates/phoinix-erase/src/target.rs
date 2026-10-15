//! Targets, their description and the refusal rules.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use phoinix_block::BlockReader;
use phoinix_core::ByteRange;
use phoinix_device::{DeviceInfo, DevicePath, platform_enumerator};
use phoinix_volume::read_partition_table;
use serde::{Deserialize, Serialize};

use crate::error::{EraseError, Refusal};
use crate::files::FilesDescription;
use crate::platform;
use crate::space::SpaceDescription;

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
    /// The free space of a volume (and optionally the slack of its files),
    /// found through the read-only filesystem engines and written raw.
    /// The volume must not be mounted (Windows dismounts it; on Linux the
    /// user unmounts it first).
    FreeSpace {
        /// Device or image path.
        path: PathBuf,
        /// Partition index (1-based), or `None` for a bare volume or the
        /// only partition.
        #[serde(default)]
        partition: Option<u32>,
        /// Also overwrite the slack of the allocated files.
        #[serde(default)]
        slack: bool,
    },
    /// The free space of a mounted volume, through the filesystem: files
    /// are written until the volume is full, rewritten for every further
    /// pass, read back and deleted. Needs no administrator rights and
    /// leaves the volume in use. What it cannot reach is stated in
    /// [`crate::MOUNTED_NOTE`].
    MountedFreeSpace {
        /// Mount point or drive (`E:\`, `/media/user/stick`).
        mount: PathBuf,
        /// Bytes to leave free (0: fill the volume completely).
        #[serde(default)]
        reserve: u64,
    },
    /// Files and folders: their content is overwritten through the
    /// filesystem (named streams included), the files are truncated,
    /// renamed and deleted, the folders removed. What this cannot reach is
    /// stated in [`crate::FILES_NOTE`].
    Files {
        /// Files and folders; folders are walked, symbolic links are not
        /// followed.
        paths: Vec<PathBuf>,
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
            | Self::File { path }
            | Self::FreeSpace { path, .. } => path,
            Self::MountedFreeSpace { mount, .. } => mount,
            Self::Files { paths } => paths.first().map_or(Path::new(""), PathBuf::as_path),
        }
    }

    /// Whether the target is written through the filesystem (a mounted
    /// volume's free space, or files) rather than as a raw range.
    #[must_use]
    pub const fn is_mounted(&self) -> bool {
        matches!(self, Self::MountedFreeSpace { .. } | Self::Files { .. })
    }

    /// Short kind name.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::WholeDevice { .. } => "device",
            Self::Partition { .. } => "partition",
            Self::Range { .. } => "range",
            Self::File { .. } => "file",
            Self::FreeSpace { .. } => "free-space",
            Self::MountedFreeSpace { .. } => "mounted",
            Self::Files { .. } => "files",
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
    /// The ranges to write, as absolute offsets on the path, sorted and
    /// merged. Empty for a single contiguous target (`offset`/`length`).
    #[serde(default)]
    pub ranges: Vec<ByteRange>,
    /// The volume behind a free-space target.
    #[serde(default)]
    pub space: Option<SpaceDescription>,
    /// Things the user should know that are not refusals.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// The files and folders of a files target.
    #[serde(default)]
    pub files: Option<FilesDescription>,
}

impl TargetDescription {
    /// The byte ranges to write, absolute on the path, in order.
    #[must_use]
    pub fn spans(&self) -> Vec<ByteRange> {
        if self.ranges.is_empty() {
            vec![ByteRange {
                offset: self.offset,
                length: self.length,
            }]
        } else {
            self.ranges.clone()
        }
    }

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
        if let Some(sp) = &self.space {
            parts.push(format!(
                "{} free space{}",
                sp.filesystem,
                if sp.slack { " + slack" } else { "" }
            ));
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
    if let EraseTarget::MountedFreeSpace { mount, reserve } = target {
        return describe_mounted(target, mount, *reserve, guards, refusals);
    }
    if let EraseTarget::Files { paths } = target {
        return describe_files(target, paths, guards);
    }
    let is_file = matches!(target, EraseTarget::File { .. })
        || (matches!(target, EraseTarget::FreeSpace { .. })
            && std::fs::metadata(&path).is_ok_and(|m| m.is_file()));
    if is_file {
        let meta = std::fs::metadata(&path).map_err(|e| EraseError::io(&path, e))?;
        if !meta.is_file() {
            return Err(EraseError::Invalid(format!(
                "{} is not a regular file; use the device or partition target",
                path.display()
            )));
        }
        let (confirmation, confirmation_short) = confirmation_for(None, &path);
        let mut desc = TargetDescription {
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
            ranges: Vec::new(),
            space: None,
            warnings: Vec::new(),
            files: None,
        };
        if let EraseTarget::FreeSpace {
            partition, slack, ..
        } = target
        {
            let reader = phoinix_device::open_source(&path)?;
            let table = read_partition_table(&*reader).ok();
            desc.partitions = table
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
            describe_free_space(&mut desc, reader, *partition, *slack)?;
        }
        return Ok(desc);
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
        // The free space is resolved below once the guards are known; a
        // partition index is checked here.
        EraseTarget::FreeSpace {
            partition: Some(index),
            ..
        } => match partitions.iter().find(|p| p.index == *index) {
            Some(p) => (p.offset, p.length, Some(p.clone())),
            None => {
                refusals.push(Refusal::NoSuchPartition { index: *index });
                (0, 0, None)
            }
        },
        EraseTarget::FreeSpace { .. } => (0, device_len, None),
        // Files and mounted volumes are described above; keep the match
        // exhaustive.
        EraseTarget::File { .. }
        | EraseTarget::MountedFreeSpace { .. }
        | EraseTarget::Files { .. } => (0, 0, None),
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
    let mut desc = TargetDescription {
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
        ranges: Vec::new(),
        space: None,
        warnings: Vec::new(),
        files: None,
    };
    if let EraseTarget::FreeSpace {
        partition, slack, ..
    } = target
        && !desc
            .refusals
            .iter()
            .any(|r| matches!(r, Refusal::NoSuchPartition { .. }))
    {
        describe_free_space(&mut desc, reader, *partition, *slack)?;
    }
    Ok(desc)
}

/// Resolves the free space of the chosen volume into `desc.ranges`.
fn describe_free_space(
    desc: &mut TargetDescription,
    reader: Arc<dyn BlockReader>,
    partition: Option<u32>,
    slack: bool,
) -> Result<(), EraseError> {
    let chosen = match partition {
        Some(index) => match desc.partitions.iter().find(|p| p.index == index).cloned() {
            Some(p) => Some(p),
            None => {
                desc.refusals.push(Refusal::NoSuchPartition { index });
                desc.length = 0;
                return Ok(());
            }
        },
        None => match desc.partitions.len() {
            0 => None,
            1 => desc.partitions.first().cloned(),
            n => {
                return Err(EraseError::Invalid(format!(
                    "the source has {n} partitions; say which one (--partition)"
                )));
            }
        },
    };
    let (volume_offset, volume_reader): (u64, Arc<dyn BlockReader>) = match &chosen {
        Some(p) => (
            p.offset,
            Arc::new(
                phoinix_block::SubrangeReader::with_bounds(reader, p.offset, p.length)
                    .map_err(|e| EraseError::Invalid(e.to_string()))?,
            ),
        ),
        None => (0, reader),
    };
    let (space, ranges) = crate::space::free_space(volume_reader, volume_offset, slack)?;
    if !space.map_available {
        desc.refusals.push(Refusal::Unsupported {
            reason: format!(
                "the {} volume has no readable allocation map, so its free space cannot be told from its files",
                space.filesystem
            ),
        });
    }
    desc.partition = chosen;
    desc.offset = volume_offset;
    desc.length = ranges.iter().map(|r| r.length).sum();
    desc.ranges = ranges;
    desc.warnings.push(crate::FREE_SPACE_NOTE.to_owned());
    if slack && space.slack_ranges == 0 {
        desc.warnings.push(
            "No file slack was found (or the engine cannot list the allocated files).".into(),
        );
    }
    desc.space = Some(space);
    Ok(())
}

/// Describes files and folders: walks them, finds their volume, and
/// refuses the running executable and read-only volumes.
fn describe_files(
    target: &EraseTarget,
    paths: &[PathBuf],
    guards: &Guards,
) -> Result<TargetDescription, EraseError> {
    if paths.is_empty() {
        return Err(EraseError::Invalid("no files given".into()));
    }
    let files = crate::files::collect(paths)?;
    let mut refusals = Vec::new();
    let mut warnings = vec![crate::FILES_NOTE.to_owned()];
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| std::fs::canonicalize(e).ok());
    if let Some(exe) = &exe
        && files
            .entries
            .iter()
            .any(|e| std::fs::canonicalize(&e.path).ok().as_ref() == Some(exe))
    {
        refusals.push(Refusal::Unsupported {
            reason: "the running PhoinixDR executable is among the files".into(),
        });
    }
    for s in &files.skipped {
        warnings.push(format!("Skipped: {s}"));
    }
    // The volume holding the first path (every path should be on the same
    // volume; the certificate names the first).
    let first = paths.first().map_or(Path::new(""), PathBuf::as_path);
    let canonical = std::fs::canonicalize(first).unwrap_or_else(|_| first.to_path_buf());
    let volume = platform::mounted_volumes()
        .into_iter()
        .filter(|v| {
            let root = std::fs::canonicalize(&v.mount).unwrap_or_else(|_| PathBuf::from(&v.mount));
            canonical.starts_with(&root)
        })
        .max_by_key(|v| v.mount.len());
    if let Some(v) = &volume
        && v.read_only
    {
        refusals.push(Refusal::Unsupported {
            reason: format!("{} is mounted read-only", v.mount),
        });
    }
    let disk = volume
        .as_ref()
        .and_then(|v| v.disk.clone())
        .or_else(|| platform::disk_of_path(first));
    if let Some(d) = &disk
        && guards
            .open_sources
            .iter()
            .any(|s| platform::disk_of_path(s).is_some_and(|sd| platform::same_disk(&sd, d)))
    {
        refusals.push(Refusal::OpenSource);
    }
    let device = disk.as_deref().and_then(|d| {
        platform_enumerator()
            .enumerate()
            .ok()?
            .into_iter()
            .find(|x| platform::same_disk(x.path.as_str(), d))
    });
    if files.entries.is_empty() {
        warnings.push(
            "No regular files found under the given paths; only the folders would be removed."
                .into(),
        );
    }
    let confirmation = if paths.len() == 1 {
        first
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "root".into())
    } else {
        format!("{} items", paths.len())
    };
    let length = crate::files::bytes_per_pass(&files);
    Ok(TargetDescription {
        target: target.clone(),
        path: canonical.display().to_string(),
        disk,
        solid_state: device.as_ref().and_then(|d| d.rotational.map(|r| !r)),
        device,
        offset: 0,
        length,
        sector_size: 1,
        partition: None,
        partitions: Vec::new(),
        volumes: Vec::new(),
        refusals,
        confirmation,
        confirmation_short: None,
        ranges: Vec::new(),
        space: volume.map(|v| SpaceDescription {
            filesystem: v.filesystem.clone(),
            volume_offset: 0,
            volume_length: v.total,
            cluster_size: files.allocation_unit,
            map_available: true,
            free_bytes: v.free,
            free_ranges: 0,
            slack: true,
            slack_bytes: 0,
            slack_ranges: 0,
            mount: Some(v.mount.clone()),
            node: Some(v.node.clone()),
            label: v.label.clone(),
            total_bytes: Some(v.total),
        }),
        warnings,
        files: Some(files),
    })
}

/// Describes a mounted volume for the filesystem-level free-space wipe.
fn describe_mounted(
    target: &EraseTarget,
    mount: &Path,
    reserve: u64,
    guards: &Guards,
    mut refusals: Vec<Refusal>,
) -> Result<TargetDescription, EraseError> {
    let canonical = std::fs::canonicalize(mount).unwrap_or_else(|_| mount.to_path_buf());
    if !canonical.is_dir() {
        return Err(EraseError::Invalid(format!(
            "{} is not a directory; pass the drive (E:\\), the mount point or a folder on the volume",
            mount.display()
        )));
    }
    // The volume holding the directory: the mount with the longest prefix.
    let found = platform::mounted_volumes()
        .into_iter()
        .filter(|v| {
            let root = std::fs::canonicalize(&v.mount).unwrap_or_else(|_| PathBuf::from(&v.mount));
            canonical.starts_with(&root)
        })
        .max_by_key(|v| v.mount.len());
    let Some(v) = found else {
        return Err(EraseError::Invalid(format!(
            "{} is not on a mounted volume this process can see",
            mount.display()
        )));
    };
    if v.read_only {
        refusals.push(Refusal::Unsupported {
            reason: format!("{} is mounted read-only", v.mount),
        });
    }
    let disk = v.disk.clone().or_else(|| platform::disk_of_path(mount));
    if let Some(d) = &disk
        && guards
            .open_sources
            .iter()
            .any(|s| platform::disk_of_path(s).is_some_and(|sd| platform::same_disk(&sd, d)))
    {
        refusals.push(Refusal::OpenSource);
    }
    let device = disk.as_deref().and_then(|d| {
        platform_enumerator()
            .enumerate()
            .ok()?
            .into_iter()
            .find(|x| platform::same_disk(x.path.as_str(), d))
    });
    let mut warnings = vec![crate::MOUNTED_NOTE.to_owned()];
    if platform::system_disk()
        .is_some_and(|s| disk.as_deref().is_some_and(|d| platform::same_disk(&s, d)))
        || v.system
    {
        warnings.push("This volume belongs to the system disk. While it is full the system and other programs may misbehave; close them first, and let the wipe finish (the files are removed at the end).".into());
    }
    let last_component = |text: &str| -> Option<String> {
        let t = text.trim_end_matches(['\\', '/']);
        t.rsplit(['\\', '/'])
            .next()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
    };
    let confirmation = v
        .label
        .clone()
        .filter(|l| l.trim().len() >= 2)
        .or_else(|| last_component(&v.mount))
        .or_else(|| last_component(&v.node))
        .unwrap_or_else(|| "root".to_owned());
    let free = v.free.saturating_sub(reserve);
    Ok(TargetDescription {
        target: target.clone(),
        path: canonical.display().to_string(),
        disk,
        solid_state: device.as_ref().and_then(|d| d.rotational.map(|r| !r)),
        device,
        offset: 0,
        length: free,
        sector_size: 1,
        partition: None,
        partitions: Vec::new(),
        volumes: Vec::new(),
        refusals,
        confirmation,
        confirmation_short: None,
        ranges: Vec::new(),
        space: Some(SpaceDescription {
            filesystem: v.filesystem.clone(),
            volume_offset: 0,
            volume_length: v.total,
            cluster_size: 0,
            map_available: true,
            free_bytes: free,
            free_ranges: 0,
            slack: false,
            slack_bytes: 0,
            slack_ranges: 0,
            mount: Some(v.mount.clone()),
            node: Some(v.node.clone()),
            label: v.label.clone(),
            total_bytes: Some(v.total),
        }),
        warnings,
        files: None,
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
