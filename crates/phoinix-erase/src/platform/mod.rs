//! Platform code: which disk a path lives on, mounted volumes, and opening
//! a target for writing. The only place that opens devices with write
//! access.

use std::path::Path;

use crate::error::EraseError;
use crate::target::TargetDescription;
use crate::writer::RangeWriter;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(not(any(target_os = "linux", windows)))]
mod other;
#[cfg(not(any(target_os = "linux", windows)))]
use other as imp;

/// A volume the operating system has mounted from a disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeOnDisk {
    /// Volume node (`/dev/sdb1`, `\\?\Volume{...}`).
    pub node: String,
    /// Mount point or drive letter.
    pub mount: String,
    /// Byte offset on the disk, if known.
    pub offset: Option<u64>,
    /// Length, if known.
    pub length: Option<u64>,
    /// Whether the erase can lock and dismount it itself.
    pub can_dismount: bool,
}

/// A volume the operating system has mounted, as a target of the
/// filesystem-level free-space wipe.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MountedVolumeInfo {
    /// Mount point or drive (`E:\\`, `/media/user/stick`).
    pub mount: String,
    /// Volume node (`\\.\E:`, `/dev/sdb1`).
    pub node: String,
    /// Filesystem name as the operating system reports it.
    pub filesystem: String,
    /// Volume label, if any.
    pub label: Option<String>,
    /// Total bytes.
    pub total: u64,
    /// Free bytes available to this process.
    pub free: u64,
    /// The whole disk the volume lies on, when known.
    pub disk: Option<String>,
    /// Whether the volume is mounted read-only.
    pub read_only: bool,
    /// Whether the volume holds the running operating system.
    pub system: bool,
}

/// Every writable volume the operating system has mounted from a disk.
#[must_use]
pub fn mounted_volumes() -> Vec<MountedVolumeInfo> {
    imp::mounted_volumes()
}

/// Total and free bytes of the volume holding `path`.
///
/// # Errors
///
/// Returns the I/O error of the platform query.
pub fn free_space(path: &Path) -> std::io::Result<(u64, u64)> {
    imp::free_space(path)
}

/// The whole-disk device path that `path` (a file or directory) lives on.
#[must_use]
pub fn disk_of_path(path: &Path) -> Option<String> {
    imp::disk_of_path(path)
}

/// The disk hosting the running operating system.
#[must_use]
pub fn system_disk() -> Option<String> {
    imp::system_disk()
}

/// Mounted volumes on `disk`.
#[must_use]
pub fn volumes_on(disk: &str) -> Vec<VolumeOnDisk> {
    imp::volumes_on(disk)
}

/// Whether two disk paths name the same disk.
#[must_use]
pub fn same_disk(a: &str, b: &str) -> bool {
    imp::same_disk(a, b)
}

/// Opens the described target for writing, locking and dismounting the
/// volumes on it where the platform allows.
///
/// # Errors
///
/// Returns [`EraseError`] when the target cannot be opened or a volume
/// cannot be dismounted.
pub fn open_writer(desc: &TargetDescription) -> Result<Box<dyn RangeWriter>, EraseError> {
    imp::open_writer(desc)
}
