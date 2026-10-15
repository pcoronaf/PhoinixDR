//! Windows: disks through `IOCTL_STORAGE_GET_DEVICE_NUMBER`, volumes through
//! the volume enumeration API and their disk extents, writes through
//! `\\.\PhysicalDriveN` opened with `GENERIC_WRITE`, after locking and
//! dismounting every volume that overlaps the target.
//!
//! `DeviceIoControl` and the volume enumeration calls are the only
//! `unsafe` code in this crate; each call site documents its contract.

#![allow(unsafe_code)]

use std::fs::File;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;

use phoinix_core::bytes::ByteView;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_READ, FILE_SHARE_WRITE, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose,
    GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    GetVolumePathNameW, GetVolumePathNamesForVolumeNameW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_STORAGE_GET_DEVICE_NUMBER,
};

use crate::error::{EraseError, Refusal};
use crate::platform::{MountedVolumeInfo, VolumeOnDisk};
use crate::target::{EraseTarget, TargetDescription};
use crate::writer::{FileWriter, RangeWriter};

fn ioctl(file: &File, code: u32, output: &mut [u8]) -> std::io::Result<usize> {
    let out_len =
        u32::try_from(output.len()).map_err(|_| std::io::Error::other("ioctl output too large"))?;
    let mut returned: u32 = 0;
    let out_ptr = if output.is_empty() {
        ptr::null_mut()
    } else {
        output.as_mut_ptr().cast()
    };
    // SAFETY: `file` owns a valid handle for the call's duration; `out_ptr`
    // and `out_len` describe a live buffer that outlives the call; no
    // overlapped structure is passed, so the call is synchronous.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            ptr::null(),
            0,
            out_ptr,
            out_len,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(usize::try_from(returned)
        .unwrap_or(usize::MAX)
        .min(output.len()))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(buf.get(..end).unwrap_or(&[]))
}

/// Disk number of the volume or device behind `handle`.
fn device_number(file: &File) -> Option<u32> {
    let mut out = [0u8; 12];
    let n = ioctl(file, IOCTL_STORAGE_GET_DEVICE_NUMBER, &mut out).ok()?;
    if n < 8 {
        return None;
    }
    ByteView::new(&out).u32_le(4)
}

fn open_volume(volume: &str) -> std::io::Result<File> {
    File::options()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(volume)
}

/// The volume GUID path (`\\?\Volume{...}\`) for every volume on the system.
fn all_volumes() -> Vec<String> {
    let mut names = Vec::new();
    let mut buf = vec![0u16; 260];
    // SAFETY: `buf` is a live, correctly sized UTF-16 buffer for the call.
    let handle = unsafe { FindFirstVolumeW(buf.as_mut_ptr(), buf.len() as u32) };
    if handle == INVALID_HANDLE_VALUE {
        return names;
    }
    names.push(from_wide(&buf));
    loop {
        // SAFETY: `handle` came from `FindFirstVolumeW` and is closed below;
        // `buf` is live and correctly sized.
        let ok = unsafe { FindNextVolumeW(handle, buf.as_mut_ptr(), buf.len() as u32) };
        if ok == 0 {
            break;
        }
        names.push(from_wide(&buf));
    }
    // SAFETY: closing the handle obtained above exactly once.
    unsafe { FindVolumeClose(handle) };
    names
}

/// Drive letters / mount folders of a volume GUID path.
fn mount_points(volume: &str) -> Vec<String> {
    let name = wide(volume);
    let mut buf = vec![0u16; 1024];
    let mut len: u32 = 0;
    // SAFETY: `name` is NUL-terminated; `buf` is live and its length is passed.
    let ok = unsafe {
        GetVolumePathNamesForVolumeNameW(
            name.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
            &mut len,
        )
    };
    if ok == 0 {
        return Vec::new();
    }
    let used = usize::try_from(len).unwrap_or(0).min(buf.len());
    buf.get(..used)
        .unwrap_or(&[])
        .split(|c| *c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

/// (disk number, offset, length) extents of a volume.
fn extents(file: &File) -> Vec<(u32, u64, u64)> {
    let mut out = vec![0u8; 8 + 24 * 64];
    let Ok(n) = ioctl(file, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, &mut out) else {
        return Vec::new();
    };
    let view = ByteView::new(out.get(..n).unwrap_or(&[]));
    let count = view.u32_le(0).unwrap_or(0) as usize;
    (0..count.min(64))
        .filter_map(|i| {
            let base = 8 + i * 24;
            Some((
                view.u32_le(base)?,
                view.u64_le(base + 8)?,
                view.u64_le(base + 16)?,
            ))
        })
        .collect()
}

fn disk_number_of(disk: &str) -> Option<u32> {
    let text = disk.trim();
    let digits = text.rsplit(|c: char| !c.is_ascii_digit()).next()?;
    if text.to_ascii_lowercase().contains("physicaldrive") {
        digits.parse().ok()
    } else {
        None
    }
}

pub fn disk_of_path(path: &Path) -> Option<String> {
    let text = path.to_string_lossy();
    if let Some(n) = disk_number_of(&text) {
        return Some(format!(r"\\.\PhysicalDrive{n}"));
    }
    let name = wide(&text);
    let mut buf = vec![0u16; 260];
    // SAFETY: `name` is NUL-terminated; `buf` is live and its length is passed.
    let ok = unsafe { GetVolumePathNameW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    if ok == 0 {
        return None;
    }
    let root = from_wide(&buf);
    let trimmed = root.trim_end_matches('\\');
    let file = open_volume(&format!(r"\\.\{trimmed}")).ok()?;
    device_number(&file).map(|n| format!(r"\\.\PhysicalDrive{n}"))
}

pub fn system_disk() -> Option<String> {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    disk_of_path(Path::new(&root))
}

pub fn volumes_on(disk: &str) -> Vec<VolumeOnDisk> {
    let Some(number) = disk_number_of(disk) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for guid in all_volumes() {
        let handle_path = guid.trim_end_matches('\\').to_owned();
        let Ok(file) = open_volume(&handle_path) else {
            continue;
        };
        for (n, offset, length) in extents(&file) {
            if n != number {
                continue;
            }
            let mounts = mount_points(&guid);
            out.push(VolumeOnDisk {
                node: handle_path.clone(),
                mount: mounts
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "(no drive letter)".to_owned()),
                offset: Some(offset),
                length: Some(length),
                can_dismount: true,
            });
        }
    }
    out
}

pub fn free_space(path: &Path) -> std::io::Result<(u64, u64)> {
    let name = wide(&path.to_string_lossy());
    let mut avail: u64 = 0;
    let mut total: u64 = 0;
    let mut free: u64 = 0;
    // SAFETY: `name` is NUL-terminated and the three out-pointers are live
    // `u64`s for the call's duration.
    let ok = unsafe { GetDiskFreeSpaceExW(name.as_ptr(), &mut avail, &mut total, &mut free) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((total, avail))
}

/// Drive type constants (`winbase.h`).
const DRIVE_REMOVABLE: u32 = 2;
const DRIVE_FIXED: u32 = 3;
const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;

pub fn mounted_volumes() -> Vec<MountedVolumeInfo> {
    let system = system_disk();
    let system_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    let system_letter = system_root.chars().next().map(|c| c.to_ascii_uppercase());
    // SAFETY: no arguments; returns a bitmask.
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = char::from(b'A' + u8::try_from(i).unwrap_or(0));
        let root = format!("{letter}:\\");
        let root_w = wide(&root);
        // SAFETY: `root_w` is NUL-terminated.
        let kind = unsafe { GetDriveTypeW(root_w.as_ptr()) };
        if kind != DRIVE_FIXED && kind != DRIVE_REMOVABLE {
            continue;
        }
        let mut label = vec![0u16; 261];
        let mut fsname = vec![0u16; 261];
        let mut serial: u32 = 0;
        let mut max_len: u32 = 0;
        let mut flags: u32 = 0;
        // SAFETY: every buffer is live and its length is passed; the
        // out-pointers are live `u32`s.
        let ok = unsafe {
            GetVolumeInformationW(
                root_w.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                &mut serial,
                &mut max_len,
                &mut flags,
                fsname.as_mut_ptr(),
                fsname.len() as u32,
            )
        };
        if ok == 0 {
            continue;
        }
        let Ok((total, free)) = free_space(Path::new(&root)) else {
            continue;
        };
        let disk = disk_of_path(Path::new(&root));
        let label = from_wide(&label);
        out.push(MountedVolumeInfo {
            mount: root.clone(),
            node: format!(r"\\.\{letter}:"),
            filesystem: from_wide(&fsname),
            label: (!label.is_empty()).then_some(label),
            total,
            free,
            system: system_letter == Some(letter)
                || system
                    .as_deref()
                    .zip(disk.as_deref())
                    .is_some_and(|(s, d)| same_disk(s, d) && system_letter == Some(letter)),
            disk,
            read_only: flags & FILE_READ_ONLY_VOLUME != 0,
        });
    }
    out
}

pub fn same_disk(a: &str, b: &str) -> bool {
    match (disk_number_of(a), disk_number_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// Locks and dismounts the volume; the returned handle keeps the lock.
fn lock_and_dismount(node: &str) -> std::io::Result<File> {
    let file = open_volume(node)?;
    ioctl(&file, FSCTL_LOCK_VOLUME, &mut [])?;
    ioctl(&file, FSCTL_DISMOUNT_VOLUME, &mut [])?;
    Ok(file)
}

pub fn open_writer(desc: &TargetDescription) -> Result<Box<dyn RangeWriter>, EraseError> {
    let path = desc.target.path();
    if let EraseTarget::File { .. } = desc.target {
        let file = File::options()
            .write(true)
            .open(path)
            .map_err(|e| EraseError::io(path, e))?;
        return Ok(Box::new(FileWriter::new(file, path, Vec::new())));
    }
    let mut locks = Vec::new();
    for v in &desc.volumes {
        match lock_and_dismount(&v.node) {
            Ok(h) => locks.push(h),
            Err(e) => {
                tracing::warn!(volume = %v.node, mount = %v.mount, error = %e, "cannot lock volume");
                return Err(EraseError::Refused(Refusal::Mounted {
                    mount: v.mount.clone(),
                }));
            }
        }
    }
    let file = File::options()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)
        .map_err(|e| EraseError::io(path, e))?;
    Ok(Box::new(FileWriter::new(file, path, locks)))
}
