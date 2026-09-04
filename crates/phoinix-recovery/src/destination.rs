//! Destination safety checks.

use std::path::Path;

use phoinix_device::{DiskIdentity, disk_of_path, disk_of_source};
use serde::{Deserialize, Serialize};

/// Outcome of comparing a destination with the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestinationCheck {
    /// Whole disk behind the source, if determined.
    pub source_disk: Option<DiskIdentity>,
    /// Whole disk behind the destination, if determined.
    pub destination_disk: Option<DiskIdentity>,
    /// `Some(true)` when both disks were determined and are the same.
    pub same_disk: Option<bool>,
    /// The destination path is the source image itself or lies inside it.
    pub overwrites_source_image: bool,
    /// Whether the source is an image file rather than a device.
    pub source_is_image: bool,
}

impl DestinationCheck {
    /// Whether recovery must be refused by default.
    ///
    /// Writing onto the disk being recovered from can overwrite the very
    /// clusters that still hold the lost data. For image-file sources the
    /// same disk is acceptable (the image is a file, not the evidence media)
    /// as long as the image itself is not overwritten.
    #[must_use]
    pub const fn is_dangerous(&self) -> bool {
        self.overwrites_source_image
            || (!self.source_is_image && matches!(self.same_disk, Some(true)))
    }

    /// Human-readable explanation of the danger, if any.
    #[must_use]
    pub fn warning(&self) -> Option<String> {
        if self.overwrites_source_image {
            return Some("The recovery destination is the source image itself; writing there would destroy the evidence.".into());
        }
        if !self.source_is_image && self.same_disk == Some(true) {
            return Some(
                "The selected recovery destination is located on the disk being recovered. Writing here may permanently overwrite recoverable data."
                    .into(),
            );
        }
        if !self.source_is_image && self.same_disk.is_none() {
            return Some("PHOINIX could not determine whether the destination lies on the source disk; verify it manually.".into());
        }
        None
    }
}

/// Compares `destination` (a directory, possibly not yet existing) with the
/// source at `source_path`.
#[must_use]
pub fn check_destination(source_path: &Path, destination: &Path) -> DestinationCheck {
    let source_is_image = std::fs::metadata(source_path)
        .map(|m| m.is_file())
        .unwrap_or(false);
    let source_disk = disk_of_source(source_path);
    let destination_disk = disk_of_path(destination);
    let same_disk = match (&source_disk, &destination_disk) {
        (Some(a), Some(b)) => Some(a == b),
        _ => None,
    };
    let overwrites_source_image = source_is_image && {
        let src = std::fs::canonicalize(source_path).ok();
        let dst = existing_ancestor(destination).and_then(|p| std::fs::canonicalize(p).ok());
        match (src, dst) {
            (Some(s), Some(d)) => d == s || d.starts_with(&s),
            _ => false,
        }
    };
    DestinationCheck {
        source_disk,
        destination_disk,
        same_disk,
        overwrites_source_image,
        source_is_image,
    }
}

fn existing_ancestor(path: &Path) -> Option<&Path> {
    let mut p = path;
    loop {
        if p.exists() {
            return Some(p);
        }
        p = p.parent()?;
    }
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
    fn image_source_next_to_destination_is_fine_but_image_itself_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("disk.img");
        std::fs::write(&image, b"x").unwrap();
        let check = check_destination(&image, &dir.path().join("out"));
        assert!(check.source_is_image);
        assert!(!check.overwrites_source_image);
        assert!(!check.is_dangerous(), "{check:?}");
        let check = check_destination(&image, &image);
        assert!(check.overwrites_source_image);
        assert!(check.is_dangerous());
        assert!(check.warning().unwrap().contains("source image"));
    }
}
