//! RAW (dd-style) image files and other plain file-backed sources.

use std::fs::File;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use phoinix_core::SourceId;

use crate::{BlockError, BlockGeometry, BlockReader, check_request};

/// A [`BlockReader`] over a plain file: a RAW/dd image, or a device node
/// opened by the device layer.
///
/// Reads use positional OS calls (`pread` on Unix, `ReadFile` with an explicit
/// offset on Windows) so concurrent readers never race on a shared cursor.
#[derive(Debug)]
pub struct RawImage {
    id: SourceId,
    path: PathBuf,
    file: File,
    length: u64,
    geometry: BlockGeometry,
}

impl RawImage {
    /// Opens an image file read-only with default 512-byte sector geometry.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::SourceUnavailable`], [`BlockError::PermissionDenied`]
    /// or [`BlockError::Io`].
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BlockError> {
        Self::open_with_geometry(path, BlockGeometry::SECTOR_512)
    }

    /// Opens an image file read-only with explicit geometry.
    ///
    /// # Errors
    ///
    /// As [`open`](Self::open).
    pub fn open_with_geometry(
        path: impl AsRef<Path>,
        geometry: BlockGeometry,
    ) -> Result<Self, BlockError> {
        let path = path.as_ref();
        let file = File::options().read(true).open(path)?;
        let length = file.metadata()?.len();
        tracing::info!(path = %path.display(), length, "opened RAW image");
        Ok(Self::from_file(file, path.to_path_buf(), length, geometry))
    }

    /// Wraps an already-open, read-only file whose length is known.
    ///
    /// Device layers use this when `metadata().len()` does not describe the
    /// underlying media (for example a Linux block device node).
    #[must_use]
    pub fn from_file(file: File, path: PathBuf, length: u64, geometry: BlockGeometry) -> Self {
        Self {
            id: SourceId::new(),
            path,
            file,
            length,
            geometry,
        }
    }

    /// Path this reader was opened from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(unix)]
    fn os_read_at(&self, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
        use std::os::unix::fs::FileExt;
        self.file.read_at(buffer, offset)
    }

    #[cfg(windows)]
    fn os_read_at(&self, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
        use std::os::windows::fs::FileExt;
        self.file.seek_read(buffer, offset)
    }
}

impl BlockReader for RawImage {
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
        self.path.display().to_string()
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, BlockError> {
        check_request(self.length, offset, buffer.len())?;
        if buffer.is_empty() {
            return Ok(0);
        }
        tracing::trace!(offset, length = buffer.len(), "raw read");
        loop {
            match self.os_read_at(offset, buffer) {
                Ok(n) => return Ok(n),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }
}
