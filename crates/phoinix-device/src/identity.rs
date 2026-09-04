//! Identifying which physical disk a path lives on.
//!
//! The recovery writer uses this to refuse destinations that resolve to the
//! disk being recovered from (ADR-0007).

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::DevicePath;

/// The whole disk behind a path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiskIdentity {
    /// Platform path of the whole disk (`/dev/sda`, `\\.\PhysicalDrive0`).
    pub disk: DevicePath,
}

/// Returns the whole disk holding the filesystem that contains `path`
/// (a file or directory), if it can be determined.
#[must_use]
pub fn disk_of_path(path: &Path) -> Option<DiskIdentity> {
    #[cfg(target_os = "linux")]
    {
        linux::disk_of_path(path)
    }
    #[cfg(windows)]
    {
        windows::disk_of_path(path)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = path;
        None
    }
}

/// Returns the whole disk a *source* path refers to: for a device node the
/// disk itself (or the disk a partition node belongs to); for an image file
/// the disk holding the image.
#[must_use]
pub fn disk_of_source(path: &Path) -> Option<DiskIdentity> {
    #[cfg(target_os = "linux")]
    {
        linux::disk_of_source(path)
    }
    #[cfg(windows)]
    {
        windows::disk_of_source(path)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = path;
        None
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use std::path::{Path, PathBuf};

    use super::DiskIdentity;
    use crate::DevicePath;

    fn major(dev: u64) -> u64 {
        ((dev >> 32) & 0xFFFF_F000) | ((dev >> 8) & 0xFFF)
    }

    fn minor(dev: u64) -> u64 {
        ((dev >> 12) & 0xFFFF_FF00) | (dev & 0xFF)
    }

    /// Resolves a block device number to its whole-disk `/dev` path via sysfs.
    fn disk_for_dev(dev: u64) -> Option<DiskIdentity> {
        let sys = PathBuf::from(format!("/sys/dev/block/{}:{}", major(dev), minor(dev)));
        let real = std::fs::canonicalize(&sys).ok()?;
        let disk_dir = if real.join("partition").exists() {
            real.parent()?.to_path_buf()
        } else {
            real
        };
        // Device-mapper and md devices report their backing disks under
        // `slaves/`; use the first one so LVM-on-disk still matches.
        let slaves = disk_dir.join("slaves");
        if let Ok(mut entries) = std::fs::read_dir(&slaves)
            && let Some(Ok(first)) = entries.next()
        {
            let slave = std::fs::canonicalize(first.path()).ok()?;
            let slave_disk = if slave.join("partition").exists() {
                slave.parent()?.to_path_buf()
            } else {
                slave
            };
            let name = slave_disk.file_name()?.to_string_lossy().into_owned();
            return Some(DiskIdentity {
                disk: DevicePath::new(format!("/dev/{name}")),
            });
        }
        let name = disk_dir.file_name()?.to_string_lossy().into_owned();
        Some(DiskIdentity {
            disk: DevicePath::new(format!("/dev/{name}")),
        })
    }

    pub fn disk_of_path(path: &Path) -> Option<DiskIdentity> {
        // Walk up until an existing ancestor is found (the destination may
        // not exist yet).
        let mut probe = path.to_path_buf();
        loop {
            if let Ok(meta) = std::fs::metadata(&probe) {
                return disk_for_dev(meta.dev());
            }
            probe = probe.parent()?.to_path_buf();
        }
    }

    pub fn disk_of_source(path: &Path) -> Option<DiskIdentity> {
        let meta = std::fs::metadata(path).ok()?;
        if meta.file_type().is_block_device() {
            return disk_for_dev(meta.rdev());
        }
        disk_for_dev(meta.dev())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use std::fs::File;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::ptr;

    use phoinix_core::bytes::ByteView;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetVolumePathNameW,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::IOCTL_STORAGE_GET_DEVICE_NUMBER;

    use super::DiskIdentity;
    use crate::DevicePath;

    fn device_number(handle_path: &str) -> Option<u32> {
        let file = File::options()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(handle_path)
            .ok()?;
        // STORAGE_DEVICE_NUMBER { DeviceType: u32, DeviceNumber: u32, PartitionNumber: u32 }
        let mut out = [0u8; 12];
        let mut returned: u32 = 0;
        // SAFETY: `file` is a valid open handle for the duration of the call;
        // `out` is a live 12-byte buffer matching STORAGE_DEVICE_NUMBER and the
        // kernel writes at most `out.len()` bytes into it; the call is
        // synchronous (no OVERLAPPED), so nothing is referenced afterwards.
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                IOCTL_STORAGE_GET_DEVICE_NUMBER,
                ptr::null(),
                0,
                out.as_mut_ptr().cast(),
                12,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok == 0 || returned < 8 {
            return None;
        }
        ByteView::new(&out).u32_le(4)
    }

    fn physical_drive(n: u32) -> DiskIdentity {
        DiskIdentity {
            disk: DevicePath::new(format!(r"\\.\PhysicalDrive{n}")),
        }
    }

    pub fn disk_of_path(path: &Path) -> Option<DiskIdentity> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut root = vec![0u16; 1024];
        // SAFETY: `wide` is NUL-terminated and outlives the call; `root` is a
        // live buffer whose length is passed as the capacity.
        let ok = unsafe { GetVolumePathNameW(wide.as_ptr(), root.as_mut_ptr(), 1024) };
        if ok == 0 {
            return None;
        }
        let len = root.iter().position(|c| *c == 0)?;
        let mut root_str = String::from_utf16_lossy(root.get(..len)?);
        while root_str.ends_with('\\') {
            root_str.pop();
        }
        // `C:` → `\\.\C:` ; `\\?\Volume{guid}` → `\\.\Volume{guid}`
        let handle = if root_str.starts_with(r"\\?\") {
            root_str.replacen(r"\\?\", r"\\.\", 1)
        } else {
            format!(r"\\.\{root_str}")
        };
        device_number(&handle).map(physical_drive)
    }

    pub fn disk_of_source(path: &Path) -> Option<DiskIdentity> {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\.\PhysicalDrive")
            && let Ok(n) = rest.parse::<u32>()
        {
            return Some(physical_drive(n));
        }
        if text.starts_with(r"\\.\") {
            return device_number(&text).map(physical_drive);
        }
        disk_of_path(path)
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_destination_walks_up_to_an_existing_ancestor() {
        // The root filesystem exists; the identity may be None on exotic
        // roots (overlayfs, tmpfs) but must not panic.
        let a = disk_of_path(Path::new("/definitely/not/here/yet"));
        let b = disk_of_path(Path::new("/"));
        assert_eq!(a, b);
    }
}
