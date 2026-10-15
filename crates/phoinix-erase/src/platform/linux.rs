//! Linux: disks via sysfs device numbers, mounts via `/proc/self/mountinfo`,
//! writes through the block device node. Mounted volumes are never
//! dismounted by PhoinixDR; the user unmounts them first.
//!
//! `statvfs` is the only `unsafe` call in this module; its contract is
//! documented at the call site.

#![allow(unsafe_code)]

use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::error::{EraseError, Refusal};
use crate::platform::{MountedVolumeInfo, VolumeOnDisk};
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

/// Mount entries from `/proc/self/mountinfo` whose source is a block
/// device: (mount point, source node, filesystem, read-only).
fn block_mounts() -> Vec<(String, String, String, bool)> {
    let Ok(info) = fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in info.lines() {
        let mut halves = line.splitn(2, " - ");
        let left = halves.next().unwrap_or("");
        let right = halves.next().unwrap_or("");
        let mut fields = left.split(' ');
        let mount = fields.nth(4).unwrap_or("").replace("\\040", " ");
        let options = fields.next().unwrap_or("");
        let read_only = options.split(',').any(|o| o == "ro");
        let mut rfields = right.split(' ');
        let fstype = rfields.next().unwrap_or("").to_owned();
        let source = rfields.next().unwrap_or("").to_owned();
        if !source.starts_with("/dev/") {
            continue;
        }
        out.push((mount, source, fstype, read_only));
    }
    out
}

pub fn free_space(path: &Path) -> std::io::Result<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::other("path contains a NUL byte"))?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid NUL-terminated path for the call's duration
    // and `st` is a live, properly sized `statvfs` the kernel fills in.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &raw mut st) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let frsize = st.f_frsize.max(1);
    Ok((
        st.f_blocks.saturating_mul(frsize),
        st.f_bavail.saturating_mul(frsize),
    ))
}

fn label_of(node: &str) -> Option<String> {
    let target = fs::canonicalize(node).ok()?;
    for entry in fs::read_dir("/dev/disk/by-label").ok()?.flatten() {
        if fs::canonicalize(entry.path()).ok()? == target {
            return Some(
                entry
                    .file_name()
                    .to_string_lossy()
                    .replace("\\x20", " ")
                    .replace("\\x2f", "/"),
            );
        }
    }
    None
}

pub fn mounted_volumes() -> Vec<MountedVolumeInfo> {
    let system = system_disk();
    let mut out = Vec::new();
    for (mount, source, fstype, read_only) in block_mounts() {
        if matches!(
            fstype.as_str(),
            "squashfs" | "iso9660" | "udf" | "fuseblk.iso"
        ) {
            continue;
        }
        let Ok(meta) = fs::metadata(&source) else {
            continue;
        };
        if !meta.file_type().is_block_device() {
            continue;
        }
        let disk = disk_of_dev(meta.rdev());
        let Ok((total, free)) = free_space(Path::new(&mount)) else {
            continue;
        };
        // One entry per source node: bind mounts repeat the same volume.
        if out.iter().any(|v: &MountedVolumeInfo| v.node == source) {
            continue;
        }
        out.push(MountedVolumeInfo {
            mount: mount.clone(),
            node: source.clone(),
            filesystem: fstype,
            label: label_of(&source),
            total,
            free,
            system: mount == "/"
                || system
                    .as_deref()
                    .zip(disk.as_deref())
                    .is_some_and(|(s, d)| same_disk(s, d))
                    && mount == "/",
            disk,
            read_only,
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
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| EraseError::io(path, e))?;
    Ok(Box::new(FileWriter::new(file, path, Vec::new())))
}
