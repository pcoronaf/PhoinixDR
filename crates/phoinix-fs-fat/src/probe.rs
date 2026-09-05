//! Native FAT probe.

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::FileSystemType;
use phoinix_fs::{FileSystemProbe, FsError, ProbeEvidence, ProbeResult};

use crate::FatBootSector;

/// Recognises FAT12/16/32 by fully validating the BPB and reading the FAT
/// media byte.
#[derive(Debug, Default, Clone, Copy)]
pub struct FatProbe;

impl FileSystemProbe for FatProbe {
    fn filesystem(&self) -> FileSystemType {
        FileSystemType::Fat32
    }

    fn probe(&self, reader: &dyn BlockReader) -> Result<ProbeResult, FsError> {
        if reader.len() < 512 {
            return Ok(ProbeResult::negative(
                FileSystemType::Fat32,
                "source shorter than a boot sector",
            ));
        }
        let sector = reader.read_vec(0, 512)?;
        let boot = match FatBootSector::parse(&sector) {
            Ok(b) => b,
            Err(e) => {
                return Ok(ProbeResult::negative(
                    FileSystemType::Fat32,
                    format!("not a FAT boot sector: {e}"),
                ));
            }
        };
        let fs = boot.variant.filesystem_type();
        let mut evidence = vec![
            ProbeEvidence::supports("BIOS Parameter Block fields are valid"),
            ProbeEvidence::supports(format!(
                "{} data clusters imply {}",
                boot.cluster_count, boot.variant
            )),
            ProbeEvidence::supports("55 AA boot signature present"),
        ];
        let mut confidence = 85;
        // The first FAT entry must start with the media descriptor.
        if reader.len() >= boot.fat_offset.saturating_add(4) {
            let first = reader.read_vec(boot.fat_offset, 4)?;
            if first.first() == Some(&boot.media) {
                evidence.push(ProbeEvidence::supports(
                    "FAT[0] carries the media descriptor",
                ));
                confidence = 92;
            } else {
                evidence.push(ProbeEvidence::contradicts(
                    "FAT[0] does not carry the media descriptor",
                ));
                confidence = 60;
            }
        }
        if boot.volume_bytes() > reader.len() {
            evidence.push(ProbeEvidence::contradicts(
                "declared size exceeds the source",
            ));
            confidence = confidence.min(70);
        }
        if !boot.type_label.is_empty() {
            evidence.push(ProbeEvidence::supports(format!(
                "type label {:?}",
                boot.type_label
            )));
        }
        Ok(ProbeResult {
            filesystem: fs,
            confidence,
            evidence,
        })
    }
}
