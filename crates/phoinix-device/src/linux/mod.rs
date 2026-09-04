//! Linux block-device enumeration via sysfs and read-only access via `/dev`.
//!
//! No external commands (`lsblk`, `blkid`, `fdisk`) are executed; everything
//! comes from `/sys/class/block` and the device nodes themselves.

use std::fs::{self, File};
use std::io::{Seek, SeekFrom};
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use phoinix_block::{BlockGeometry, BlockReader, RawImage};
use phoinix_core::SourceId;

use crate::{
    DeviceBus, DeviceEnumerator, DeviceError, DeviceInfo, DeviceKind, DevicePath, device_source_id,
};

const SYS_BLOCK: &str = "/sys/class/block";

/// Linux implementation of [`DeviceEnumerator`].
#[derive(Debug, Clone)]
pub struct LinuxEnumerator {
    sys_block: PathBuf,
    dev_dir: PathBuf,
}

impl LinuxEnumerator {
    /// Enumerator using the real `/sys` and `/dev`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sys_block: PathBuf::from(SYS_BLOCK),
            dev_dir: PathBuf::from("/dev"),
        }
    }

    /// Enumerator rooted at alternative directories (for tests).
    #[must_use]
    pub fn with_roots(sys_block: PathBuf, dev_dir: PathBuf) -> Self {
        Self { sys_block, dev_dir }
    }

    fn describe(&self, name: &str) -> Result<Option<DeviceInfo>, DeviceError> {
        if name.starts_with("ram") || name.starts_with("zram") {
            return Ok(None);
        }
        let sys = self.sys_block.join(name);
        let size_sectors = read_u64(&sys.join("size")).unwrap_or(0);
        // sysfs `size` is always in 512-byte units regardless of logical block size.
        let size = size_sectors
            .checked_mul(512)
            .ok_or_else(|| DeviceError::Malformed {
                device: name.to_owned(),
                detail: "size overflows".into(),
            })?;
        if size == 0 && (name.starts_with("loop") || name.starts_with("sr")) {
            // Detached loop devices and empty optical drives are noise.
            return Ok(None);
        }

        let is_partition = sys.join("partition").exists();
        let real = fs::canonicalize(&sys).unwrap_or_else(|_| sys.clone());
        let disk_sys = if is_partition {
            real.parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| real.clone())
        } else {
            real.clone()
        };
        let queue = disk_sys.join("queue");

        let logical = read_u32(&queue.join("logical_block_size")).unwrap_or(512);
        let physical = read_u32(&queue.join("physical_block_size"));
        let mut geometry = BlockGeometry::new(logical).unwrap_or(BlockGeometry::SECTOR_512);
        if let Some(p) = physical
            && let Ok(g) = geometry.clone().with_physical(p)
        {
            geometry = g;
        }
        let rotational = read_u32(&queue.join("rotational")).map(|v| v == 1);
        let removable = read_u32(&disk_sys.join("removable")).map(|v| v == 1);

        let device_link = disk_sys.join("device");
        let vendor = read_trimmed(&device_link.join("vendor"));
        let model = read_trimmed(&device_link.join("model"));
        let serial = read_trimmed(&device_link.join("serial"))
            .or_else(|| read_trimmed(&device_link.join("wwid")));
        let bus = Some(classify_bus(name, &real, &device_link));

        let node = self.dev_dir.join(name);
        let accessible = File::options().read(true).open(&node).is_ok();
        let path = DevicePath::new(node.to_string_lossy().into_owned());
        let parent = if is_partition {
            disk_sys
                .file_name()
                .map(|n| DevicePath::new(self.dev_dir.join(n).to_string_lossy().into_owned()))
        } else {
            None
        };
        let display_name = match (&vendor, &model) {
            (Some(v), Some(m)) if !v.is_empty() => format!("{v} {m}"),
            (_, Some(m)) => m.clone(),
            _ => name.to_owned(),
        };

        Ok(Some(DeviceInfo {
            id: device_source_id(&path),
            path,
            display_name,
            kind: if is_partition {
                DeviceKind::Partition
            } else {
                DeviceKind::Disk
            },
            parent,
            size,
            geometry,
            removable,
            rotational,
            bus,
            vendor,
            model,
            serial,
            accessible,
        }))
    }

    fn open_node(&self, path: &Path) -> Result<Arc<dyn BlockReader>, DeviceError> {
        let file = File::options()
            .read(true)
            .open(path)
            .map_err(|e| map_open_error(e, path))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let sys = self.sys_block.join(&name);
        let mut length = read_u64(&sys.join("size"))
            .and_then(|s| s.checked_mul(512))
            .unwrap_or(0);
        let mut geometry = BlockGeometry::SECTOR_512;
        if let Ok(Some(info)) = self.describe(&name) {
            geometry = info.geometry;
            if info.size > 0 {
                length = info.size;
            }
        }
        if length == 0 {
            // Fall back to the kernel's idea of the end of the device.
            let mut probe = File::options()
                .read(true)
                .open(path)
                .map_err(|e| map_open_error(e, path))?;
            length = probe.seek(SeekFrom::End(0))?;
        }
        tracing::info!(path = %path.display(), length, sector = geometry.logical_sector_size, "opened block device read-only");
        Ok(Arc::new(RawImage::from_file(
            file,
            path.to_path_buf(),
            length,
            geometry,
        )))
    }
}

impl Default for LinuxEnumerator {
    fn default() -> Self {
        Self::new()
    }
}

impl DeviceEnumerator for LinuxEnumerator {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        let entries = match fs::read_dir(&self.sys_block) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        let mut devices = Vec::new();
        for name in names {
            match self.describe(&name) {
                Ok(Some(info)) => devices.push(info),
                Ok(None) => {}
                Err(e) => tracing::warn!(device = %name, error = %e, "skipping device"),
            }
        }
        // Whole disks first, then partitions, each group by path.
        devices.sort_by(|a, b| {
            a.kind
                .cmp(&b.kind)
                .then_with(|| a.path.as_str().cmp(b.path.as_str()))
        });
        Ok(devices)
    }

    fn open_readonly(&self, id: &SourceId) -> Result<Arc<dyn BlockReader>, DeviceError> {
        let devices = self.enumerate()?;
        let info = devices
            .into_iter()
            .find(|d| &d.id == id)
            .ok_or_else(|| DeviceError::NotFound(id.to_string()))?;
        self.open_node(&info.path.to_path_buf())
    }

    fn open_path_readonly(&self, path: &DevicePath) -> Result<Arc<dyn BlockReader>, DeviceError> {
        self.open_node(&path.to_path_buf())
    }

    fn is_device_path(&self, path: &Path) -> bool {
        fs::metadata(path)
            .map(|m| m.file_type().is_block_device())
            .unwrap_or(false)
    }
}

fn map_open_error(err: std::io::Error, path: &Path) -> DeviceError {
    match err.kind() {
        std::io::ErrorKind::NotFound => DeviceError::NotFound(path.display().to_string()),
        std::io::ErrorKind::PermissionDenied => {
            DeviceError::PermissionDenied(path.display().to_string())
        }
        _ => DeviceError::Io(err),
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

fn read_u32(path: &Path) -> Option<u32> {
    read_trimmed(path)?.parse().ok()
}

fn classify_bus(name: &str, real_sys_path: &Path, device_link: &Path) -> DeviceBus {
    let sys = real_sys_path.to_string_lossy();
    if sys.contains("/devices/virtual/")
        || name.starts_with("loop")
        || name.starts_with("dm-")
        || name.starts_with("md")
    {
        return DeviceBus::Virtual;
    }
    let dev = fs::canonicalize(device_link)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let hay = format!("{sys} {dev}");
    if hay.contains("/nvme/") || name.starts_with("nvme") {
        DeviceBus::Nvme
    } else if hay.contains("/usb") {
        DeviceBus::Usb
    } else if hay.contains("/mmc") || name.starts_with("mmcblk") {
        DeviceBus::Sd
    } else if hay.contains("/ata") {
        DeviceBus::Sata
    } else if hay.contains("/virtio") || name.starts_with("vd") || name.starts_with("xvd") {
        DeviceBus::Virtual
    } else if hay.contains("/host") && (hay.contains("sas") || hay.contains("/expander")) {
        DeviceBus::Sas
    } else {
        DeviceBus::Unknown
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]

    use super::*;

    fn fake_sysfs() -> (tempfile::TempDir, LinuxEnumerator) {
        let dir = tempfile::tempdir().unwrap();
        let sys = dir.path().join("sys/class/block");
        let dev = dir.path().join("dev");
        fs::create_dir_all(&dev).unwrap();
        // Disk sda: 4K physical / 512 logical, rotational, with a partition.
        let sda = sys.join("sda");
        fs::create_dir_all(sda.join("queue")).unwrap();
        fs::create_dir_all(sda.join("device")).unwrap();
        fs::write(sda.join("size"), "2000409264\n").unwrap();
        fs::write(sda.join("removable"), "0\n").unwrap();
        fs::write(sda.join("queue/logical_block_size"), "512\n").unwrap();
        fs::write(sda.join("queue/physical_block_size"), "4096\n").unwrap();
        fs::write(sda.join("queue/rotational"), "1\n").unwrap();
        fs::write(sda.join("device/vendor"), "ATA     \n").unwrap();
        fs::write(sda.join("device/model"), "WDC WD10EZEX\n").unwrap();
        fs::write(sda.join("device/serial"), "WD-123\n").unwrap();
        let sda1 = sda.join("sda1");
        fs::create_dir_all(&sda1).unwrap();
        fs::write(sda1.join("size"), "2048\n").unwrap();
        fs::write(sda1.join("partition"), "1\n").unwrap();
        // Expose the partition at the class level as sysfs does (symlink).
        std::os::unix::fs::symlink(&sda1, sys.join("sda1")).unwrap();
        // A detached loop device that must be skipped.
        let loop0 = sys.join("loop0");
        fs::create_dir_all(&loop0).unwrap();
        fs::write(loop0.join("size"), "0\n").unwrap();
        // A zram device that must be skipped.
        let zram = sys.join("zram0");
        fs::create_dir_all(&zram).unwrap();
        fs::write(zram.join("size"), "1024\n").unwrap();
        (dir, LinuxEnumerator::with_roots(sys, dev))
    }

    #[test]
    fn enumerates_disks_and_partitions_from_sysfs() {
        let (_dir, e) = fake_sysfs();
        let devices = e.enumerate().unwrap();
        assert_eq!(devices.len(), 2, "{devices:?}");
        let disk = &devices[0];
        assert_eq!(disk.kind, DeviceKind::Disk);
        assert_eq!(disk.size, 2_000_409_264 * 512);
        assert_eq!(disk.geometry.logical_sector_size, 512);
        assert_eq!(disk.geometry.physical_sector_size, Some(4096));
        assert_eq!(disk.rotational, Some(true));
        assert_eq!(disk.removable, Some(false));
        assert_eq!(disk.model.as_deref(), Some("WDC WD10EZEX"));
        assert_eq!(disk.serial.as_deref(), Some("WD-123"));
        assert_eq!(disk.display_name, "ATA WDC WD10EZEX");
        let part = &devices[1];
        assert_eq!(part.kind, DeviceKind::Partition);
        assert_eq!(part.size, 2048 * 512);
        assert_eq!(
            part.geometry.physical_sector_size,
            Some(4096),
            "partition inherits disk geometry"
        );
        assert!(part.parent.as_ref().unwrap().as_str().ends_with("/sda"));
        assert_eq!(part.id, device_source_id(&part.path));
    }

    #[test]
    fn unknown_id_is_not_found() {
        let (_dir, e) = fake_sysfs();
        assert!(matches!(
            e.open_readonly(&SourceId::nil()),
            Err(DeviceError::NotFound(_))
        ));
    }
}
