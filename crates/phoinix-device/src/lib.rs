//! Physical device enumeration and read-only device access.
//!
//! This is the only PHOINIX crate besides FFI adapters that may contain
//! `unsafe` code, and only inside the Windows platform module where
//! `DeviceIoControl` is required to query device length and geometry. Every
//! unsafe block documents its invariants. Devices are always opened without
//! write access (ADR-0007).
//!
//! The public surface is platform-neutral:
//!
//! - [`DeviceInfo`] describes an enumerated device;
//! - [`DeviceEnumerator`] lists devices and opens them read-only;
//! - [`platform_enumerator`] returns the enumerator for the current OS;
//! - [`open_source`] opens either a device path or an image file as a
//!   [`BlockReader`].

#![deny(unsafe_code)]

mod error;
mod model;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(windows)]
pub mod windows;

use std::path::Path;
use std::sync::Arc;

use phoinix_block::{BlockReader, RawImage};

pub use error::DeviceError;
pub use model::{DeviceBus, DeviceInfo, DeviceKind, DevicePath, device_source_id};

/// Enumerates block devices and opens them read-only.
pub trait DeviceEnumerator: Send + Sync {
    /// Lists block devices visible to this process.
    ///
    /// Devices that cannot be fully described (for example because of
    /// insufficient privileges) are still listed with unknown fields where
    /// possible.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError`] if the platform device registry is
    /// unavailable.
    fn enumerate(&self) -> Result<Vec<DeviceInfo>, DeviceError>;

    /// Opens the device with the given identifier read-only.
    ///
    /// Identifiers are stable per path (see [`device_source_id`]), so a value
    /// printed by an earlier `enumerate` remains valid.
    ///
    /// # Errors
    ///
    /// Returns [`DeviceError::NotFound`] if no such device exists,
    /// [`DeviceError::PermissionDenied`] if elevation is required, or another
    /// [`DeviceError`].
    fn open_readonly(
        &self,
        id: &phoinix_core::SourceId,
    ) -> Result<Arc<dyn BlockReader>, DeviceError>;

    /// Opens a device by platform path read-only.
    ///
    /// # Errors
    ///
    /// As [`open_readonly`](Self::open_readonly).
    fn open_path_readonly(&self, path: &DevicePath) -> Result<Arc<dyn BlockReader>, DeviceError>;

    /// Whether `path` names a block device on this platform (as opposed to a
    /// regular file).
    fn is_device_path(&self, path: &Path) -> bool;
}

/// Returns the device enumerator for the current platform.
///
/// On platforms without an implementation a [`NoDevices`] enumerator is
/// returned so that image-file workflows keep working.
#[must_use]
pub fn platform_enumerator() -> Box<dyn DeviceEnumerator> {
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxEnumerator::new())
    }
    #[cfg(windows)]
    {
        Box::new(windows::WindowsEnumerator::new())
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Box::new(NoDevices)
    }
}

/// Enumerator for platforms without device support.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoDevices;

impl DeviceEnumerator for NoDevices {
    fn enumerate(&self) -> Result<Vec<DeviceInfo>, DeviceError> {
        Ok(Vec::new())
    }

    fn open_readonly(
        &self,
        id: &phoinix_core::SourceId,
    ) -> Result<Arc<dyn BlockReader>, DeviceError> {
        Err(DeviceError::NotFound(id.to_string()))
    }

    fn open_path_readonly(&self, path: &DevicePath) -> Result<Arc<dyn BlockReader>, DeviceError> {
        Err(DeviceError::Unsupported(format!(
            "device access is not supported on this platform: {path}"
        )))
    }

    fn is_device_path(&self, _path: &Path) -> bool {
        false
    }
}

/// Opens `path` read-only as a block source: a physical device when the path
/// names one on this platform, otherwise a RAW image file.
///
/// # Errors
///
/// Returns [`DeviceError`] if the path cannot be opened.
pub fn open_source(path: impl AsRef<Path>) -> Result<Arc<dyn BlockReader>, DeviceError> {
    let path = path.as_ref();
    let enumerator = platform_enumerator();
    if enumerator.is_device_path(path) {
        tracing::info!(path = %path.display(), "opening block device read-only");
        return enumerator.open_path_readonly(&DevicePath::from(path));
    }
    let image = RawImage::open(path)?;
    Ok(Arc::new(image))
}
