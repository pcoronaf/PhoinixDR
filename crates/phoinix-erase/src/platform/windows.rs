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
    FILE_SHARE_READ, FILE_SHARE_WRITE, FindClose, FindFirstStreamW, FindFirstVolumeW,
    FindNextStreamW, FindNextVolumeW, FindStreamInfoStandard, FindVolumeClose, GetDiskFreeSpaceExW,
    GetDiskFreeSpaceW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW, GetVolumePathNameW,
    GetVolumePathNamesForVolumeNameW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, WIN32_FIND_STREAM_DATA,
};
use windows_sys::Win32::Storage::IscsiDisc::{
    ATA_FLAGS_48BIT_COMMAND, ATA_FLAGS_DATA_IN, ATA_FLAGS_DATA_OUT, ATA_FLAGS_DRDY_REQUIRED,
    ATA_PASS_THROUGH_EX, IOCTL_ATA_PASS_THROUGH,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_STORAGE_GET_DEVICE_NUMBER,
    IOCTL_STORAGE_PROTOCOL_COMMAND, ProtocolTypeNvme, STORAGE_PROTOCOL_COMMAND,
    STORAGE_PROTOCOL_COMMAND_FLAG_ADAPTER_REQUEST, STORAGE_PROTOCOL_COMMAND_LENGTH_NVME,
    STORAGE_PROTOCOL_SPECIFIC_NVME_ADMIN_COMMAND, STORAGE_PROTOCOL_STRUCTURE_VERSION,
};

use crate::error::{EraseError, Refusal};
use crate::firmware::{AtaCommand, AtaResult, Backend, DataDir, NvmeCommand, Transport};
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

/// Named streams of a file (`:name:$DATA`), the unnamed one left out.
pub fn file_streams(path: &Path) -> Vec<crate::files::StreamEntry> {
    let name = wide(&path.to_string_lossy());
    let mut data = WIN32_FIND_STREAM_DATA::default();
    // SAFETY: `name` is NUL-terminated and `data` is a live, correctly
    // typed buffer for the standard info level; the handle is closed below.
    let handle = unsafe {
        FindFirstStreamW(
            name.as_ptr(),
            FindStreamInfoStandard,
            (&raw mut data).cast(),
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Vec::new();
    }
    let mut out = Vec::new();
    loop {
        let stream = from_wide(&data.cStreamName);
        // `::$DATA` is the unnamed stream; `:name:$DATA` a named one.
        if let Some(rest) = stream.strip_prefix(':')
            && let Some((n, kind)) = rest.rsplit_once(':')
            && !n.is_empty()
            && kind.eq_ignore_ascii_case("$DATA")
        {
            out.push(crate::files::StreamEntry {
                name: n.to_owned(),
                size: u64::try_from(data.StreamSize).unwrap_or(0),
            });
        }
        // SAFETY: `handle` came from `FindFirstStreamW`; `data` is live.
        let ok = unsafe { FindNextStreamW(handle, (&raw mut data).cast()) };
        if ok == 0 {
            break;
        }
    }
    // SAFETY: closing the search handle obtained above exactly once.
    unsafe { FindClose(handle) };
    out
}

pub fn allocation_unit(path: &Path) -> Option<u64> {
    let name = wide(&path.to_string_lossy());
    let mut root = vec![0u16; 260];
    // SAFETY: `name` is NUL-terminated; `root` is live and its length is passed.
    let ok = unsafe { GetVolumePathNameW(name.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
    if ok == 0 {
        return None;
    }
    let mut spc: u32 = 0;
    let mut bps: u32 = 0;
    let mut free: u32 = 0;
    let mut total: u32 = 0;
    // SAFETY: `root` is NUL-terminated and the out-pointers are live `u32`s.
    let ok = unsafe { GetDiskFreeSpaceW(root.as_ptr(), &mut spc, &mut bps, &mut free, &mut total) };
    if ok == 0 {
        return None;
    }
    Some(u64::from(spc).saturating_mul(u64::from(bps)).max(1))
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

// ---------------------------------------------------------------------------
// Firmware pass-through: IOCTL_ATA_PASS_THROUGH and
// IOCTL_STORAGE_PROTOCOL_COMMAND (NVMe admin commands).

/// The pass-through over an open device handle.
pub struct WindowsBackend {
    file: File,
    transport: Transport,
}

/// A DeviceIoControl with one buffer used for input and output.
fn ioctl_inout(file: &File, code: u32, buf: &mut [u8]) -> std::io::Result<usize> {
    let len = u32::try_from(buf.len()).map_err(|_| std::io::Error::other("buffer too large"))?;
    let mut returned: u32 = 0;
    // SAFETY: `file` owns a valid handle; `buf` is a live buffer used as
    // both input and output for the synchronous call.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            buf.as_mut_ptr().cast(),
            len,
            buf.as_mut_ptr().cast(),
            len,
            &mut returned,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(usize::try_from(returned).unwrap_or(0).min(buf.len()))
}

impl Backend for WindowsBackend {
    fn transport(&self) -> Transport {
        self.transport
    }

    fn ata(
        &mut self,
        cmd: &AtaCommand,
        data: &mut [u8],
        timeout: std::time::Duration,
    ) -> std::io::Result<AtaResult> {
        let header = size_of::<ATA_PASS_THROUGH_EX>();
        let data_len = match cmd.dir {
            DataDir::None => 0,
            _ => data.len(),
        };
        let mut buf = vec![0u8; header + data_len];
        let mut apt = ATA_PASS_THROUGH_EX::default();
        apt.Length = u16::try_from(header).unwrap_or(0);
        apt.AtaFlags = u16::try_from(
            ATA_FLAGS_DRDY_REQUIRED
                | match cmd.dir {
                    DataDir::None => 0,
                    DataDir::In => ATA_FLAGS_DATA_IN,
                    DataDir::Out => ATA_FLAGS_DATA_OUT,
                }
                | if cmd.ext { ATA_FLAGS_48BIT_COMMAND } else { 0 },
        )
        .unwrap_or(0);
        apt.DataTransferLength = u32::try_from(data_len).unwrap_or(0);
        apt.TimeOutValue = u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX).max(1);
        apt.DataBufferOffset = header;
        // CurrentTaskFile: features, count, lba low, mid, high, device,
        // command; PreviousTaskFile: the high bytes of the 48-bit form.
        let lba = cmd.lba.to_le_bytes();
        let block_count = u16::try_from(data_len.div_ceil(512)).unwrap_or(u16::MAX);
        let count = if cmd.dir == DataDir::None {
            cmd.count
        } else {
            block_count
        };
        apt.CurrentTaskFile = [
            (cmd.feature & 0xFF) as u8,
            (count & 0xFF) as u8,
            lba[0],
            lba[1],
            lba[2],
            cmd.device,
            cmd.command,
            0,
        ];
        apt.PreviousTaskFile = [
            (cmd.feature >> 8) as u8,
            (count >> 8) as u8,
            lba[3],
            lba[4],
            lba[5],
            0,
            0,
            0,
        ];
        // SAFETY: `ATA_PASS_THROUGH_EX` is a plain C struct; copying its
        // bytes into the buffer head is how the ioctl expects it.
        let apt_bytes =
            unsafe { std::slice::from_raw_parts((&raw const apt).cast::<u8>(), header) };
        buf[..header].copy_from_slice(apt_bytes);
        if cmd.dir == DataDir::Out {
            buf[header..].copy_from_slice(data);
        }
        ioctl_inout(&self.file, IOCTL_ATA_PASS_THROUGH, &mut buf)?;
        if cmd.dir == DataDir::In {
            data.copy_from_slice(&buf[header..header + data_len]);
        }
        // SAFETY: reading the struct back from the buffer head, which the
        // driver filled with the same layout.
        let back: ATA_PASS_THROUGH_EX = unsafe { ptr::read_unaligned(buf.as_ptr().cast()) };
        let c = back.CurrentTaskFile;
        let p = back.PreviousTaskFile;
        Ok(crate::firmware::ata::result_from_taskfile(
            c[6],
            c[0],
            c[1],
            p[1],
            [c[2], c[3], c[4], p[2], p[3], p[4]],
        ))
    }

    fn nvme_admin(
        &mut self,
        cmd: &NvmeCommand,
        dir: DataDir,
        data: &mut [u8],
        timeout: std::time::Duration,
    ) -> std::io::Result<(u16, u32)> {
        let header = size_of::<STORAGE_PROTOCOL_COMMAND>();
        let command_len = usize::try_from(STORAGE_PROTOCOL_COMMAND_LENGTH_NVME).unwrap_or(64);
        let error_len = 64usize;
        let data_len = match dir {
            DataDir::None => 0,
            _ => data.len(),
        };
        let total = header + command_len + error_len + data_len;
        let mut buf = vec![0u8; total];
        let mut spc = STORAGE_PROTOCOL_COMMAND {
            Version: STORAGE_PROTOCOL_STRUCTURE_VERSION,
            Length: u32::try_from(header).unwrap_or(0),
            ProtocolType: ProtocolTypeNvme,
            Flags: STORAGE_PROTOCOL_COMMAND_FLAG_ADAPTER_REQUEST,
            ReturnStatus: 0,
            ErrorCode: 0,
            CommandLength: STORAGE_PROTOCOL_COMMAND_LENGTH_NVME,
            ErrorInfoLength: u32::try_from(error_len).unwrap_or(0),
            DataToDeviceTransferLength: if dir == DataDir::Out {
                u32::try_from(data_len).unwrap_or(0)
            } else {
                0
            },
            DataFromDeviceTransferLength: if dir == DataDir::In {
                u32::try_from(data_len).unwrap_or(0)
            } else {
                0
            },
            TimeOutValue: u32::try_from(timeout.as_secs()).unwrap_or(u32::MAX).max(1),
            ErrorInfoOffset: u32::try_from(header + command_len).unwrap_or(0),
            DataToDeviceBufferOffset: if dir == DataDir::Out {
                u32::try_from(header + command_len + error_len).unwrap_or(0)
            } else {
                0
            },
            DataFromDeviceBufferOffset: if dir == DataDir::In {
                u32::try_from(header + command_len + error_len).unwrap_or(0)
            } else {
                0
            },
            CommandSpecific: STORAGE_PROTOCOL_SPECIFIC_NVME_ADMIN_COMMAND,
            Reserved0: 0,
            FixedProtocolReturnData: 0,
            Reserved1: [0; 3],
            Command: [0; 1],
        };
        let _ = &mut spc;
        // SAFETY: plain C struct copied into the buffer head.
        let spc_bytes =
            unsafe { std::slice::from_raw_parts((&raw const spc).cast::<u8>(), header) };
        buf[..header].copy_from_slice(spc_bytes);
        // The 64-byte NVMe submission queue entry: dword 0 = opcode (and
        // CID 0), dword 1 = NSID, dwords 10–15 as given.
        let sqe = &mut buf[header..header + command_len];
        sqe[0..4].copy_from_slice(&u32::from(cmd.opcode).to_le_bytes());
        sqe[4..8].copy_from_slice(&cmd.nsid.to_le_bytes());
        sqe[40..44].copy_from_slice(&cmd.cdw10.to_le_bytes());
        sqe[44..48].copy_from_slice(&cmd.cdw11.to_le_bytes());
        sqe[48..52].copy_from_slice(&cmd.cdw12.to_le_bytes());
        sqe[52..56].copy_from_slice(&cmd.cdw13.to_le_bytes());
        sqe[56..60].copy_from_slice(&cmd.cdw14.to_le_bytes());
        sqe[60..64].copy_from_slice(&cmd.cdw15.to_le_bytes());
        let data_offset = header + command_len + error_len;
        if dir == DataDir::Out {
            buf[data_offset..].copy_from_slice(data);
        }
        ioctl_inout(&self.file, IOCTL_STORAGE_PROTOCOL_COMMAND, &mut buf)?;
        if dir == DataDir::In {
            data.copy_from_slice(&buf[data_offset..data_offset + data_len]);
        }
        // SAFETY: the driver filled the struct at the buffer head.
        let back: STORAGE_PROTOCOL_COMMAND = unsafe { ptr::read_unaligned(buf.as_ptr().cast()) };
        // ReturnStatus 0 = success; ErrorCode carries the NVMe completion
        // status (status field in bits 16:1 of DW3 for the inbox driver).
        let status = if back.ReturnStatus == 0 {
            0
        } else {
            u16::try_from((back.ErrorCode >> 1) & 0x7FF)
                .unwrap_or(u16::MAX)
                .max(1)
        };
        Ok((status, back.FixedProtocolReturnData))
    }
}

pub fn open_firmware(
    path: &Path,
    bus: Option<phoinix_device::DeviceBus>,
) -> Option<Box<dyn Backend>> {
    let text = path.to_string_lossy();
    if disk_number_of(&text).is_none() {
        return None;
    }
    let transport = match bus {
        Some(phoinix_device::DeviceBus::Nvme) => Transport::Nvme,
        Some(phoinix_device::DeviceBus::Sata) => Transport::Ata,
        Some(phoinix_device::DeviceBus::Virtual | phoinix_device::DeviceBus::Sd) => return None,
        _ => Transport::Ata,
    };
    let file = File::options()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&*text)
        .ok()?;
    Some(Box::new(WindowsBackend { file, transport }))
}
