//! Linux: disks via sysfs device numbers, mounts via `/proc/self/mountinfo`,
//! writes through the block device node. Mounted volumes are never
//! dismounted by PhoinixDR; the user unmounts them first.
//!
//! `statvfs`, `SG_IO` and the NVMe admin ioctl are the `unsafe` calls in
//! this module; each contract is documented at the call site.

#![allow(unsafe_code)]

use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

use crate::error::{EraseError, Refusal};
use crate::firmware::{AtaCommand, AtaResult, Backend, DataDir, NvmeCommand, Transport};
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

pub fn file_streams(_path: &Path) -> Vec<crate::files::StreamEntry> {
    Vec::new()
}

pub fn allocation_unit(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: as in `free_space`: a valid NUL-terminated path and a live
    // `statvfs` the kernel fills in.
    let rc = unsafe { libc::statvfs(c.as_ptr(), &raw mut st) };
    if rc != 0 {
        return None;
    }
    Some(st.f_bsize.max(st.f_frsize).max(1))
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

// ---------------------------------------------------------------------------
// Firmware pass-through: SG_IO (ATA PASS-THROUGH 16) and NVME_IOCTL_ADMIN_CMD.

/// `SG_IO` ioctl request.
const SG_IO: libc::c_ulong = 0x2285;
/// `sg_io_hdr` interface id `'S'`.
const SG_INTERFACE_ID: libc::c_int = b'S' as libc::c_int;
const SG_DXFER_NONE: libc::c_int = -1;
const SG_DXFER_TO_DEV: libc::c_int = -2;
const SG_DXFER_FROM_DEV: libc::c_int = -3;
/// `NVME_IOCTL_ADMIN_CMD`: `_IOWR('N', 0x41, struct nvme_admin_cmd)`.
const NVME_IOCTL_ADMIN_CMD: libc::c_ulong = 0xC048_4E41;

/// `struct sg_io_hdr` (scsi/sg.h).
#[repr(C)]
struct SgIoHdr {
    interface_id: libc::c_int,
    dxfer_direction: libc::c_int,
    cmd_len: libc::c_uchar,
    mx_sb_len: libc::c_uchar,
    iovec_count: libc::c_ushort,
    dxfer_len: libc::c_uint,
    dxferp: *mut libc::c_void,
    cmdp: *mut libc::c_uchar,
    sbp: *mut libc::c_uchar,
    timeout: libc::c_uint,
    flags: libc::c_uint,
    pack_id: libc::c_int,
    usr_ptr: *mut libc::c_void,
    status: libc::c_uchar,
    masked_status: libc::c_uchar,
    msg_status: libc::c_uchar,
    sb_len_wr: libc::c_uchar,
    host_status: libc::c_ushort,
    driver_status: libc::c_ushort,
    resid: libc::c_int,
    duration: libc::c_uint,
    info: libc::c_uint,
}

/// `struct nvme_admin_cmd` (linux/nvme_ioctl.h), 72 bytes.
#[repr(C)]
struct NvmeAdminCmd {
    opcode: u8,
    flags: u8,
    rsvd1: u16,
    nsid: u32,
    cdw2: u32,
    cdw3: u32,
    metadata: u64,
    addr: u64,
    metadata_len: u32,
    data_len: u32,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
    timeout_ms: u32,
    result: u32,
}

/// The pass-through over an open device node.
pub struct LinuxBackend {
    file: fs::File,
    transport: Transport,
}

/// The 16-byte ATA PASS-THROUGH CDB (SAT) for `cmd`.
fn ata_cdb(cmd: &AtaCommand, len_blocks: u16) -> [u8; 16] {
    // Protocol: 3 = non-data, 4 = PIO data-in, 5 = PIO data-out. The
    // extend bit (bit 0 of byte 1) selects the 48-bit form.
    let protocol: u8 = match cmd.dir {
        DataDir::None => 3,
        DataDir::In => 4,
        DataDir::Out => 5,
    };
    let mut cdb = [0u8; 16];
    cdb[0] = 0x85;
    cdb[1] = (protocol << 1) | u8::from(cmd.ext);
    // t_length = 2 (sector count holds the block count), byte_block = 1,
    // t_dir = 1 for data-in; ck_cond = 1 so the registers come back.
    cdb[2] = match cmd.dir {
        DataDir::None => 0x20,
        DataDir::In => 0x2E,
        DataDir::Out => 0x26,
    };
    let [f_hi, f_lo] = cmd.feature.to_be_bytes();
    let [c_hi, c_lo] = if cmd.dir == DataDir::None {
        cmd.count.to_be_bytes()
    } else {
        len_blocks.to_be_bytes()
    };
    let lba = cmd.lba.to_le_bytes();
    cdb[3] = f_hi;
    cdb[4] = f_lo;
    cdb[5] = c_hi;
    cdb[6] = c_lo;
    cdb[7] = lba[3];
    cdb[8] = lba[0];
    cdb[9] = lba[4];
    cdb[10] = lba[1];
    cdb[11] = lba[5];
    cdb[12] = lba[2];
    cdb[13] = cmd.device;
    cdb[14] = cmd.command;
    cdb
}

/// The ATA registers from the sense data's ATA return descriptor (type
/// 0x09), when present.
fn ata_result_from_sense(sense: &[u8]) -> Option<AtaResult> {
    // Descriptor-format sense: byte 7 = additional length; descriptors
    // from byte 8. The ATA return descriptor: type 0x09, length 0x0C.
    let response_code = sense.first()? & 0x7F;
    if response_code != 0x72 && response_code != 0x73 {
        return None;
    }
    let mut i = 8usize;
    while i + 2 <= sense.len() {
        let kind = *sense.get(i)?;
        let len = usize::from(*sense.get(i + 1)?);
        if kind == 0x09 && len >= 0x0C {
            let d = sense.get(i..i + 2 + len)?;
            let b = |n: usize| d.get(n).copied().unwrap_or(0);
            // Bytes: 2 flags, 3 error, 4 count (15:8), 5 count (7:0),
            // 6 lba (31:24), 7 lba (7:0), 8 lba (39:32), 9 lba (15:8),
            // 10 lba (47:40), 11 lba (23:16), 12 device, 13 status.
            return Some(crate::firmware::ata::result_from_taskfile(
                b(13),
                b(3),
                b(5),
                b(4),
                [b(7), b(9), b(11), b(6), b(8), b(10)],
            ));
        }
        i += 2 + len;
    }
    None
}

impl Backend for LinuxBackend {
    fn transport(&self) -> Transport {
        self.transport
    }

    fn ata(
        &mut self,
        cmd: &AtaCommand,
        data: &mut [u8],
        timeout: std::time::Duration,
    ) -> std::io::Result<AtaResult> {
        use std::os::unix::io::AsRawFd;
        let blocks = u16::try_from(data.len().div_ceil(512)).unwrap_or(u16::MAX);
        let mut cdb = ata_cdb(cmd, blocks);
        let mut sense = [0u8; 32];
        let (dir, len, ptr) = match cmd.dir {
            DataDir::None => (SG_DXFER_NONE, 0, std::ptr::null_mut()),
            DataDir::In => (SG_DXFER_FROM_DEV, data.len(), data.as_mut_ptr().cast()),
            DataDir::Out => (SG_DXFER_TO_DEV, data.len(), data.as_mut_ptr().cast()),
        };
        let mut hdr = SgIoHdr {
            interface_id: SG_INTERFACE_ID,
            dxfer_direction: dir,
            cmd_len: 16,
            mx_sb_len: 32,
            iovec_count: 0,
            dxfer_len: u32::try_from(len).unwrap_or(u32::MAX),
            dxferp: ptr,
            cmdp: cdb.as_mut_ptr(),
            sbp: sense.as_mut_ptr(),
            timeout: u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX),
            flags: 0,
            pack_id: 0,
            usr_ptr: std::ptr::null_mut(),
            status: 0,
            masked_status: 0,
            msg_status: 0,
            sb_len_wr: 0,
            host_status: 0,
            driver_status: 0,
            resid: 0,
            duration: 0,
            info: 0,
        };
        // SAFETY: `hdr` and every buffer it points at (`cdb`, `sense`,
        // `data`) are live for the duration of the synchronous call, and
        // the file descriptor is open.
        let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), SG_IO, &raw mut hdr) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        if hdr.host_status != 0 {
            return Err(std::io::Error::other(format!(
                "SG_IO host status {:#06x}",
                hdr.host_status
            )));
        }
        let sense_len = usize::from(hdr.sb_len_wr).min(sense.len());
        // With ck_cond set the registers come back as CHECK CONDITION
        // with an ATA return descriptor, whether the command failed or not.
        if let Some(r) = ata_result_from_sense(sense.get(..sense_len).unwrap_or(&[])) {
            return Ok(r);
        }
        if hdr.status != 0 || hdr.driver_status != 0 {
            return Err(std::io::Error::other(format!(
                "SG_IO status {:#04x}, driver status {:#06x}, no ATA registers returned",
                hdr.status, hdr.driver_status
            )));
        }
        Ok(AtaResult::default())
    }

    fn nvme_admin(
        &mut self,
        cmd: &NvmeCommand,
        dir: DataDir,
        data: &mut [u8],
        timeout: std::time::Duration,
    ) -> std::io::Result<(u16, u32)> {
        use std::os::unix::io::AsRawFd;
        let (addr, len) = match dir {
            DataDir::None => (0u64, 0u32),
            _ => (
                data.as_mut_ptr() as u64,
                u32::try_from(data.len()).unwrap_or(u32::MAX),
            ),
        };
        let mut c = NvmeAdminCmd {
            opcode: cmd.opcode,
            flags: 0,
            rsvd1: 0,
            nsid: cmd.nsid,
            cdw2: 0,
            cdw3: 0,
            metadata: 0,
            addr,
            metadata_len: 0,
            data_len: len,
            cdw10: cmd.cdw10,
            cdw11: cmd.cdw11,
            cdw12: cmd.cdw12,
            cdw13: cmd.cdw13,
            cdw14: cmd.cdw14,
            cdw15: cmd.cdw15,
            timeout_ms: u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX),
            result: 0,
        };
        // SAFETY: `c` and the data buffer it points at are live for the
        // duration of the synchronous call; the file descriptor is open.
        let rc = unsafe { libc::ioctl(self.file.as_raw_fd(), NVME_IOCTL_ADMIN_CMD, &raw mut c) };
        if rc < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // The ioctl returns the NVMe status code field (0 = success).
        Ok((u16::try_from(rc).unwrap_or(u16::MAX), c.result))
    }
}

pub fn open_firmware(
    path: &Path,
    bus: Option<phoinix_device::DeviceBus>,
) -> Option<Box<dyn Backend>> {
    let meta = fs::metadata(path).ok()?;
    if !meta.file_type().is_block_device() {
        return None;
    }
    let name = path.file_name()?.to_string_lossy().into_owned();
    let transport = match bus {
        Some(phoinix_device::DeviceBus::Nvme) => Transport::Nvme,
        Some(phoinix_device::DeviceBus::Sata) => Transport::Ata,
        Some(phoinix_device::DeviceBus::Virtual | phoinix_device::DeviceBus::Sd) => return None,
        _ if name.starts_with("nvme") => Transport::Nvme,
        _ if name.starts_with("sd") => Transport::Ata,
        _ => return None,
    };
    let file = fs::File::options().read(true).write(true).open(path).ok()?;
    Some(Box::new(LinuxBackend { file, transport }))
}
