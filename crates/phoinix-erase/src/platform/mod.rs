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
