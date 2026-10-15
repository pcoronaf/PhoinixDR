//! Linux: disks via sysfs device numbers, mounts via `/proc/self/mountinfo`,
//! writes through the block device node. Mounted volumes are never
//! dismounted by PhoinixDR; the user unmounts them first.

use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::error::{EraseError, Refusal};
use crate::platform::VolumeOnDisk;
use crate::target::{EraseTarget, TargetDescription};
use crate::writer::{FileWriter, RangeWriter};

fn major(dev: u64) -> u64 {
    ((dev >> 8) & 0xfff) | ((dev >> 32) & !0xfff)
}

fn minor(dev: u64) -> u64 {
    (dev & 0xff) | ((dev >> 12) & !0xff)
}

/// `/dev/<disk>` for a block device number, resolving partitions to their
/// parent disk through sysfs.
fn disk_of_dev(dev: u64) -> Option<String> {
    let sys = PathBuf::from(format!("/sys/dev/block/{}:{}", major(dev), minor(dev)));
    let real = fs::canonicalize(&sys).ok()?;
    let name = if real.join("partition").exists() {
        real.parent()?.file_name()?.to_string_lossy().into_owned()
    } else {
        real.file_name()?.to_string_lossy().into_owned()
    };
    Some(format!("/dev/{name}"))
}

pub fn disk_of_path(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if meta.file_type().is_block_device() {
        return disk_of_dev(meta.rdev());
    }
    disk_of_dev(meta.dev())
}

pub fn system_disk() -> Option<String> {
    disk_of_path(Path::new("/"))
}

fn sys_u64(name: &str, field: &str) -> Option<u64> {
    fs::read_to_string(format!("/sys/class/block/{name}/{field}"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn volumes_on(disk: &str) -> Vec<VolumeOnDisk> {
    let Ok(info) = fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in info.lines() {
        // ... mount_point ... - fstype source options
        let mut halves = line.splitn(2, " - ");
        let left = halves.next().unwrap_or("");
        let right = halves.next().unwrap_or("");
        let mount = left.split(' ').nth(4).unwrap_or("").to_owned();
        let source = right.split(' ').nth(1).unwrap_or("");
        if !source.starts_with("/dev/") {
            continue;
        }
        let Ok(meta) = fs::metadata(source) else {
            continue;
        };
        if !meta.file_type().is_block_device() {
            continue;
        }
        let Some(d) = disk_of_dev(meta.rdev()) else {
            continue;
        };
        if !same_disk(&d, disk) {
            continue;
        }
        let name = Path::new(source)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let offset = sys_u64(&name, "start").map(|s| s * 512);
        let length = sys_u64(&name, "size").map(|s| s * 512);
        out.push(VolumeOnDisk {
            node: source.to_owned(),
            mount: mount.replace("\\040", " "),
            offset,
            length,
            can_dismount: false,
        });
    }
    out
}

pub fn same_disk(a: &str, b: &str) -> bool {
    let ca = fs::canonicalize(a).unwrap_or_else(|_| PathBuf::from(a));
    let cb = fs::canonicalize(b).unwrap_or_else(|_| PathBuf::from(b));
    ca == cb
}

pub fn open_writer(desc: &TargetDescription) -> Result<Box<dyn RangeWriter>, EraseError> {
    let path = desc.target.path();
    if !matches!(desc.target, EraseTarget::File { .. }) {
        // Re-check mounts right before writing: the refusal was computed
        // earlier and a volume may have been mounted since.
        if let Some(disk) = &desc.disk {
            let end = desc.offset.saturating_add(desc.length);
            if let Some(v) = volumes_on(disk)
                .into_iter()
                .find(|v| match (v.offset, v.length) {
                    (Some(o), Some(l)) => o < end && o.saturating_add(l) > desc.offset,
                    _ => true,
                })
            {
                return Err(EraseError::Refused(Refusal::Mounted { mount: v.mount }));
            }
        }
    }
    let file = fs::File::options()
        .write(true)
        .open(path)
        .map_err(|e| EraseError::io(path, e))?;
    Ok(Box::new(FileWriter::new(file, path, Vec::new())))
}
