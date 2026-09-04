//! Windows physical-drive enumeration and read-only access.
//!
//! Drives are opened through `std::fs::File` with `GENERIC_READ` only and
//! `FILE_SHARE_READ | FILE_SHARE_WRITE` so that a mounted system disk can be
//! read. `DeviceIoControl` is required to learn the device length and sector
//! geometry; those are the only `unsafe` calls in PHOINIX outside FFI
//! adapters, and each is documented at the call site.
//!
//! Windows requires reads on physical drives to be sector aligned in both
//! offset and length; [`phoinix_block::align::read_via_aligned`] handles
//! unaligned requests.

#![allow(unsafe_code)]

use std::fs::File;
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use std::ptr;
use std::sync::Arc;

use phoinix_block::align::read_via_aligned;
use phoinix_block::{BlockError, BlockGeometry, BlockReader, check_request};
use phoinix_core::SourceId;
use phoinix_core::bytes::{ByteView, ascii_field};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
};
use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_GET_LENGTH_INFO, IOCTL_STORAGE_QUERY_PROPERTY,
    PropertyStandardQuery, StorageAccessAlignmentProperty, StorageDeviceProperty,
    StorageDeviceSeekPenaltyProperty,
};

use crate::{
    DeviceBus, DeviceEnumerator, DeviceError, DeviceInfo, DeviceKind, DevicePath, device_source_id,
};

/// Highest `PhysicalDriveN` index probed during enumeration.
const MAX_PHYSICAL_DRIVES: u32 = 64;

/// Windows implementation of [`DeviceEnumerator`].
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsEnumerator;

impl WindowsEnumerator {
    /// Creates the enumerator.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

fn open_drive(path: &str) -> std::io::Result<File> {
    // Read access only; never GENERIC_WRITE (ADR-0007). Sharing both read and
    // write is required for disks that are currently mounted.
    File::options()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)
}

fn map_open_error(err: std::io::Error, path: &str) -> DeviceError {
    match err.raw_os_error().map(|c| c.unsigned_abs()) {
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => DeviceError::NotFound(path.to_owned()),
        Some(ERROR_ACCESS_DENIED) => DeviceError::PermissionDenied(path.to_owned()),
        _ => match err.kind() {
            std::io::ErrorKind::NotFound => DeviceError::NotFound(path.to_owned()),
            std::io::ErrorKind::PermissionDenied => DeviceError::PermissionDenied(path.to_owned()),
            _ => DeviceError::Io(err),
        },
    }
}

/// Issues a `DeviceIoControl` request and returns the number of output bytes.
fn ioctl(file: &File, code: u32, input: &[u8], output: &mut [u8]) -> std::io::Result<usize> {
    let in_len =
        u32::try_from(input.len()).map_err(|_| std::io::Error::other("ioctl input too large"))?;
    let out_len =
        u32::try_from(output.len()).map_err(|_| std::io::Error::other("ioctl output too large"))?;
    let mut returned: u32 = 0;
    let in_ptr = if input.is_empty() {
        ptr::null()
    } else {
        input.as_ptr().cast()
    };
    let out_ptr = if output.is_empty() {
        ptr::null_mut()
    } else {
        output.as_mut_ptr().cast()
    };
    // SAFETY: `file` owns a valid open handle for the duration of this call.
    // `in_ptr`/`in_len` and `out_ptr`/`out_len` describe live, correctly sized
    // slices that outlive the call; the kernel writes at most `out_len` bytes
    // into `output` and reports the count in `returned`. No overlapped
    // structure is passed, so the call completes synchronously and no memory
    // is referenced after it returns.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            code,
            in_ptr,
            in_len,
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

fn storage_query(file: &File, property_id: i32, output: &mut [u8]) -> std::io::Result<usize> {
    // STORAGE_PROPERTY_QUERY { PropertyId: i32, QueryType: i32, AdditionalParameters: [u8; 1] }
    let mut query = [0u8; 12];
    query[..4].copy_from_slice(&property_id.to_le_bytes());
    query[4..8].copy_from_slice(&PropertyStandardQuery.to_le_bytes());
    ioctl(file, IOCTL_STORAGE_QUERY_PROPERTY, &query, output)
}

fn query_length(file: &File) -> Option<u64> {
    // GET_LENGTH_INFORMATION { Length: i64 }
    let mut out = [0u8; 8];
    if ioctl(file, IOCTL_DISK_GET_LENGTH_INFO, &[], &mut out).ok()? >= 8 {
        let v = ByteView::new(&out).i64_le(0)?;
        return u64::try_from(v).ok();
    }
    // Fall back to DISK_GEOMETRY_EX.DiskSize (i64 at offset 24).
    let mut geo = [0u8; 64];
    let n = ioctl(file, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut geo).ok()?;
    let v = ByteView::new(geo.get(..n)?).i64_le(24)?;
    u64::try_from(v).ok()
}

fn query_geometry(file: &File) -> BlockGeometry {
    // STORAGE_ACCESS_ALIGNMENT_DESCRIPTOR: BytesPerLogicalSector @16, BytesPerPhysicalSector @20.
    let mut out = [0u8; 32];
    if let Ok(n) = storage_query(file, StorageAccessAlignmentProperty, &mut out)
        && let Some(view) = ByteView::new(&out).sub(0, n)
        && let (Some(logical), Some(physical)) = (view.u32_le(16), view.u32_le(20))
        && let Ok(g) = BlockGeometry::new(logical)
    {
        return g.clone().with_physical(physical).unwrap_or(g);
    }
    // Fall back to DISK_GEOMETRY.BytesPerSector (u32 at offset 20).
    let mut geo = [0u8; 64];
    if let Ok(n) = ioctl(file, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, &[], &mut geo)
        && let Some(view) = ByteView::new(&geo).sub(0, n)
        && let Some(bps) = view.u32_le(20)
        && let Ok(g) = BlockGeometry::new(bps)
    {
        return g;
    }
    BlockGeometry::SECTOR_512
}

struct DeviceDescriptor {
    removable: Option<bool>,
    bus: Option<DeviceBus>,
    vendor: Option<String>,
    model: Option<String>,
    serial: Option<String>,
}

fn c_string_at(view: ByteView<'_>, offset: u32) -> Option<String> {
    if offset == 0 {
        return None;
    }
    let start = usize::try_from(offset).ok()?;
    let rest = view.from(start)?;
    let text = ascii_field(rest.as_slice()).trim().to_owned();
    if text.is_empty() { None } else { Some(text) }
}

fn query_descriptor(file: &File) -> DeviceDescriptor {
    let mut out = vec![0u8; 4096];
    let Ok(n) = storage_query(file, StorageDeviceProperty, &mut out) else {
        return DeviceDescriptor {
            removable: None,
            bus: None,
            vendor: None,
            model: None,
            serial: None,
        };
    };
    let view = ByteView::new(&out).sub(0, n).unwrap_or(ByteView::new(&[]));
    // STORAGE_DEVICE_DESCRIPTOR layout: RemovableMedia @10 (BOOLEAN),
    // VendorIdOffset @12, ProductIdOffset @16, SerialNumberOffset @24, BusType @28.
    let removable = view.u8(10).map(|b| b != 0);
    let vendor = view.u32_le(12).and_then(|o| c_string_at(view, o));
    let model = view.u32_le(16).and_then(|o| c_string_at(view, o));
    let serial = view.u32_le(24).and_then(|o| c_string_at(view, o));
    let bus = view.u32_le(28).map(|b| match b {
        17 => DeviceBus::Nvme,
        2 | 3 | 11 => DeviceBus::Sata,
        1 | 8 | 9 | 10 => DeviceBus::Sas,
        7 => DeviceBus::Usb,
        12 | 13 => DeviceBus::Sd,
        14..=16 => DeviceBus::Virtual,
        _ => DeviceBus::Unknown,
    });
    DeviceDescriptor {
        removable,
        bus,
        vendor,
        model,
        serial,
    }
}

fn query_rotational(file: &File) -> Option<bool> {
    // DEVICE_SEEK_PENALTY_DESCRIPTOR { Version, Size, IncursSeekPenalty: BOOLEAN @8 }
    let mut out = [0u8; 12];
    let n = storage_query(file, StorageDeviceSeekPenaltyProperty, &mut out).ok()?;
    ByteView::new(&out).sub(0, n)?.u8(8).map(|b| b != 0)
}

fn describe(path: &str, file: &File) -> DeviceInfo {
    let device_path = DevicePath::new(path);
    let size = query_length(file).unwrap_or(0);
    let geometry = query_geometry(file);
    let desc = query_descriptor(file);
    let display_name = match (&desc.vendor, &desc.model) {
        (Some(v), Some(m)) => format!("{v} {m}"),
        (_, Some(m)) => m.clone(),
        _ => path.to_owned(),
    };
    DeviceInfo {
        id: device_source_id(&device_path),
        path: device_path,
        display_name,
        kind: DeviceKind::Disk,
        parent: None,
        size,
        geometry,
        removable: desc.removable,
        rotational: query_rotational(file),
        bus: desc.bus,
        vendor: desc.vendor,
        model: desc.model,
        serial: desc.serial,
        accessible: true,
    }
}

fn inaccessible(path: &str) -> DeviceInfo {
    let device_path = DevicePath::new(path);
    DeviceInfo {
        id: device_source_id(&device_path),
        path: device_path,
        display_name: path.to_owned(),
        kind: DeviceKind::Disk,
        parent: None,
        size: 0,
        geometry: BlockGeometry::SECTOR_512,
        removable: None,
        rotational: None,
        bus: None,
        vendor: None,
        model: None,
        serial: None,
        accessible: false,
    }
}

/// Read-only reader over a Windows physical drive or volume handle.
struct WindowsDiskReader {
    id: SourceId,
    path: String,
    file: File,
    length: u64,
    geometry: BlockGeometry,
}

impl std::fmt::Debug for WindowsDiskReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsDiskReader")
            .field("path", &self.path)
            .field("length", &self.length)
            .finish()
    }
}

impl BlockReader for WindowsDiskReader {
    fn id(&self) -> SourceId {
        self.id
    }

    fn len(&self) -> u64 {
        self.length
    }

    fn geometry(&self) -> &BlockGeometry {
        &self.geometry
    }

    fn describe(&self) -> String {
        self.path.clone()
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.length, offset, buffer.len())?;
        if buffer.is_empty() {
            return Ok(0);
        }
        let sector = self.geometry.logical_sector_size;
        let length = self.length;
        read_via_aligned(sector, offset, buffer, |aligned_offset, aligned_buf| {
            // The aligned superset may extend past the end of the device by
            // less than one sector; clamp it so the kernel never rejects it.
            let available = length.saturating_sub(aligned_offset);
            let want = aligned_buf
                .len()
                .min(usize::try_from(available).unwrap_or(usize::MAX));
            let target = aligned_buf
                .get_mut(..want)
                .ok_or(BlockError::IntegerOverflow)?;
            let mut done = 0usize;
            while done < target.len() {
                let chunk = target.get_mut(done..).ok_or(BlockError::IntegerOverflow)?;
                let pos = aligned_offset
                    .checked_add(done as u64)
                    .ok_or(BlockError::IntegerOverflow)?;
                let n = self.file.seek_read(chunk, pos)?;
                if n == 0 {
                    break;
                }
                done = done.checked_add(n).ok_or(BlockError::IntegerOverflow)?;
            }
            Ok(done)
        })
    }
}

impl DeviceEnumerator for WindowsEnumerator {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        let mut devices = Vec::new();
        for n in 0..MAX_PHYSICAL_DRIVES {
            let path = format!(r"\\.\PhysicalDrive{n}");
            match open_drive(&path) {
                Ok(file) => devices.push(describe(&path, &file)),
                Err(e) => match map_open_error(e, &path) {
                    DeviceError::NotFound(_) => {}
                    DeviceError::PermissionDenied(_) => {
                        tracing::warn!(path, "access denied; run elevated to read this device");
                        devices.push(inaccessible(&path));
                    }
                    other => tracing::warn!(path, error = %other, "skipping device"),
                },
            }
        }
        Ok(devices)
    }

    fn open_readonly(&self, id: &SourceId) -> Result<Arc<dyn BlockReader>, DeviceError> {
        for n in 0..MAX_PHYSICAL_DRIVES {
            let path = DevicePath::new(format!(r"\\.\PhysicalDrive{n}"));
            if &device_source_id(&path) == id {
                return self.open_path_readonly(&path);
            }
        }
        Err(DeviceError::NotFound(id.to_string()))
    }

    fn open_path_readonly(&self, path: &DevicePath) -> Result<Arc<dyn BlockReader>, DeviceError> {
        let file = open_drive(path.as_str()).map_err(|e| map_open_error(e, path.as_str()))?;
        let length = query_length(&file).ok_or_else(|| {
            DeviceError::Unsupported(format!("cannot determine length of {path}"))
        })?;
        let geometry = query_geometry(&file);
        tracing::info!(path = %path, length, sector = geometry.logical_sector_size, "opened device read-only");
        Ok(Arc::new(WindowsDiskReader {
            id: SourceId::new(),
            path: path.as_str().to_owned(),
            file,
            length,
            geometry,
        }))
    }

    fn is_device_path(&self, path: &Path) -> bool {
        let text = path.to_string_lossy();
        text.starts_with(r"\\.\") || text.starts_with(r"\\?\PhysicalDrive")
    }
}
