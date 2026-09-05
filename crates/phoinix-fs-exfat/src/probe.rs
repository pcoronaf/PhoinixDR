//! Native exFAT probe.

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::FileSystemType;
use phoinix_core::bytes::ByteView;
use phoinix_fs::{FileSystemProbe, FsError, ProbeEvidence, ProbeResult};

use crate::ExfatBootSector;

/// Recognises exFAT by validating the boot sector and region checksum.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExFatProbe;

impl FileSystemProbe for ExFatProbe {
    fn filesystem(&self) -> FileSystemType {
        FileSystemType::ExFat
    }

    fn probe(&self, reader: &dyn BlockReader) -> Result<ProbeResult, FsError> {
        if reader.len() < 512 {
            return Ok(ProbeResult::negative(
                FileSystemType::ExFat,
                "source shorter than a boot sector",
            ));
        }
        let sector = reader.read_vec(0, 512)?;
        if ByteView::new(&sector).slice(3, 8) != Some(b"EXFAT   ") {
            return Ok(ProbeResult::negative(
                FileSystemType::ExFat,
                "OEM name is not EXFAT",
            ));
        }
        let mut evidence = vec![ProbeEvidence::supports("OEM name is EXFAT")];
        match ExfatBootSector::parse(&sector) {
            Ok(boot) => {
                evidence.push(ProbeEvidence::supports(format!(
                    "valid geometry: {} bytes/sector, {} bytes/cluster, {} clusters",
                    boot.bytes_per_sector, boot.cluster_size, boot.cluster_count
                )));
                let mut confidence = 90;
                match boot.verify_region_checksum(reader) {
                    Some(true) => {
                        evidence.push(ProbeEvidence::supports("boot region checksum matches"));
                        confidence = 96;
                    }
                    Some(false) => {
                        evidence.push(ProbeEvidence::contradicts("boot region checksum mismatch"))
                    }
                    None => {}
                }
                if boot.volume_bytes() > reader.len() {
                    evidence.push(ProbeEvidence::contradicts(
                        "declared volume length exceeds the source",
                    ));
                    confidence = confidence.min(75);
                }
                Ok(ProbeResult {
                    filesystem: FileSystemType::ExFat,
                    confidence,
                    evidence,
                })
            }
            Err(e) => {
                evidence.push(ProbeEvidence::contradicts(format!(
                    "boot sector fails validation: {e}"
                )));
                Ok(ProbeResult {
                    filesystem: FileSystemType::ExFat,
                    confidence: 25,
                    evidence,
                })
            }
        }
    }
}
