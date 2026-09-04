//! Platform-neutral device description.

use std::fmt;
use std::path::{Path, PathBuf};

use phoinix_block::BlockGeometry;
use phoinix_core::SourceId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Namespace used to derive stable device identifiers from paths.
const DEVICE_NAMESPACE: Uuid = Uuid::from_bytes([
    0x9a, 0x1f, 0x4c, 0x2e, 0x7b, 0x0d, 0x4e, 0x3a, 0x8f, 0x61, 0x5c, 0x2b, 0x9e, 0x77, 0x01, 0x42,
]);

/// Derives a stable [`SourceId`] for a device path so that identifiers
/// printed by `phoinix devices` remain valid across invocations.
#[must_use]
pub fn device_source_id(path: &DevicePath) -> SourceId {
    SourceId::from_uuid(Uuid::new_v5(&DEVICE_NAMESPACE, path.as_str().as_bytes()))
}

/// Platform path of a device (`/dev/sda`, `\\.\PhysicalDrive0`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DevicePath(String);

impl DevicePath {
    /// Wraps a path string.
    #[must_use]
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// The path as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path as a filesystem path.
    #[must_use]
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(&self.0)
    }
}

impl fmt::Display for DevicePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&Path> for DevicePath {
    fn from(path: &Path) -> Self {
        Self(path.to_string_lossy().into_owned())
    }
}

impl From<&str> for DevicePath {
    fn from(path: &str) -> Self {
        Self(path.to_owned())
    }
}

/// Transport a device is attached through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceBus {
    /// NVMe.
    Nvme,
    /// SATA / ATA.
    Sata,
    /// SAS / SCSI.
    Sas,
    /// USB.
    Usb,
    /// SD / MMC.
    Sd,
    /// Virtual (loop, device-mapper, hypervisor disks, RAM).
    Virtual,
    /// Not determined.
    Unknown,
}

impl DeviceBus {
    /// Short label for tables.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            DeviceBus::Nvme => "NVMe",
            DeviceBus::Sata => "SATA",
            DeviceBus::Sas => "SAS",
            DeviceBus::Usb => "USB",
            DeviceBus::Sd => "SD",
            DeviceBus::Virtual => "Virtual",
            DeviceBus::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for DeviceBus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Whether a device is a whole disk or a partition exposed by the OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    /// A whole disk.
    Disk,
    /// A partition node of a disk.
    Partition,
}

/// Description of an enumerated block device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceInfo {
    /// Stable identifier derived from the path.
    pub id: SourceId,
    /// Platform path.
    pub path: DevicePath,
    /// Human-readable name (model, or the path when unknown).
    pub display_name: String,
    /// Whole disk or partition.
    pub kind: DeviceKind,
    /// For partitions, the path of the parent disk.
    pub parent: Option<DevicePath>,
    /// Size in bytes.
    pub size: u64,
    /// Sector geometry.
    pub geometry: BlockGeometry,
    /// Whether the media is removable, if known.
    pub removable: Option<bool>,
    /// Whether the media is rotational (HDD) rather than solid state, if known.
    pub rotational: Option<bool>,
    /// Transport bus, if known.
    pub bus: Option<DeviceBus>,
    /// Vendor string, if known.
    pub vendor: Option<String>,
    /// Model string, if known.
    pub model: Option<String>,
    /// Serial number, if known.
    pub serial: Option<String>,
    /// Whether this process could open the device for reading. `false`
    /// usually means elevated privileges are required.
    pub accessible: bool,
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

    #[test]
    fn ids_are_stable_per_path() {
        let a = device_source_id(&DevicePath::from("/dev/sda"));
        let b = device_source_id(&DevicePath::from("/dev/sda"));
        let c = device_source_id(&DevicePath::from("/dev/sdb"));
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
