//! Platforms without device write support: files only.

use std::path::Path;

use crate::error::{EraseError, Refusal};
use crate::platform::VolumeOnDisk;
use crate::target::{EraseTarget, TargetDescription};
use crate::writer::{FileWriter, RangeWriter};

pub fn disk_of_path(_path: &Path) -> Option<String> {
    None
}

pub fn system_disk() -> Option<String> {
    None
}

pub fn volumes_on(_disk: &str) -> Vec<VolumeOnDisk> {
    Vec::new()
}

pub fn same_disk(a: &str, b: &str) -> bool {
    a == b
}

pub fn open_writer(desc: &TargetDescription) -> Result<Box<dyn RangeWriter>, EraseError> {
    match &desc.target {
        EraseTarget::File { path } => {
            let file = std::fs::File::options()
                .write(true)
                .open(path)
                .map_err(|e| EraseError::io(path, e))?;
            Ok(Box::new(FileWriter::new(file, path, Vec::new())))
        }
        _ => Err(EraseError::Refused(Refusal::Unsupported {
            reason: "device writes are not supported on this platform".into(),
        })),
    }
}
