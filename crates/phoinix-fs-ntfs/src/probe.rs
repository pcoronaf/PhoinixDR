//! NTFS filesystem probe.

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::FileSystemType;
use phoinix_core::bytes::ByteView;
use phoinix_fs::{FileSystemProbe, FsError, ProbeEvidence, ProbeResult};

use crate::NtfsBootSector;

/// Recognises NTFS by fully validating the boot sector, not just the OEM ID.
#[derive(Debug, Default, Clone, Copy)]
pub struct NtfsProbe;

impl FileSystemProbe for NtfsProbe {
    fn filesystem(&self) -> FileSystemType {
        FileSystemType::Ntfs
    }

    fn probe(&self, reader: &dyn BlockReader) -> Result<ProbeResult, FsError> {
        if reader.len() < 512 {
            return Ok(ProbeResult::negative(
                FileSystemType::Ntfs,
                "source shorter than a boot sector",
            ));
        }
        let sector = reader.read_vec(0, 512)?;
        let view = ByteView::new(&sector);
        let oem_is_ntfs = view.slice(3, 8) == Some(b"NTFS    ");
        if !oem_is_ntfs {
            return Ok(ProbeResult::negative(
                FileSystemType::Ntfs,
                "OEM ID is not \"NTFS    \"",
            ));
        }
        let mut evidence = vec![ProbeEvidence::supports("OEM ID is \"NTFS    \"")];
        match NtfsBootSector::parse(&sector) {
            Ok(boot) => {
                evidence.push(ProbeEvidence::supports(format!(
                    "valid geometry: {} bytes/sector, {} bytes/cluster, {}-byte MFT records",
                    boot.bytes_per_sector, boot.cluster_size, boot.mft_record_size
                )));
                evidence.push(ProbeEvidence::supports("55 AA boot signature present"));
                evidence.push(ProbeEvidence::supports(format!(
                    "$MFT at LCN {} lies inside the volume",
                    boot.mft_lcn
                )));
                let mut confidence = 95;
                if boot.fits_in(reader.len()) {
                    evidence.push(ProbeEvidence::supports(format!(
                        "declared size of {} sectors fits the source",
                        boot.total_sectors
                    )));
                } else {
                    evidence.push(ProbeEvidence::contradicts(format!(
                        "declared size of {} sectors exceeds the source (truncated image or wrong partition bounds)",
                        boot.total_sectors
                    )));
                    confidence = 75;
                }
                Ok(ProbeResult {
                    filesystem: FileSystemType::Ntfs,
                    confidence,
                    evidence,
                })
            }
            Err(err) => {
                evidence.push(ProbeEvidence::contradicts(format!(
                    "boot sector fails validation: {err}"
                )));
                Ok(ProbeResult {
                    filesystem: FileSystemType::Ntfs,
                    confidence: 25,
                    evidence,
                })
            }
        }
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
    use crate::boot::testutil::{BootSpec, build};
    use phoinix_block::MemoryReader;

    #[test]
    fn strong_positive_on_valid_boot_sector() {
        let mut data = build(&BootSpec::default());
        data.resize(131_072 * 512, 0);
        let r = NtfsProbe.probe(&MemoryReader::new(data)).unwrap();
        assert_eq!(r.confidence, 95);
        assert!(r.evidence.iter().all(|e| e.supports));
    }

    #[test]
    fn truncated_source_lowers_confidence() {
        let mut data = build(&BootSpec::default());
        data.resize(4096, 0);
        let r = NtfsProbe.probe(&MemoryReader::new(data)).unwrap();
        assert_eq!(r.confidence, 75);
        assert!(r.is_positive());
    }

    #[test]
    fn oem_alone_is_not_enough() {
        let mut data = vec![0u8; 4096];
        data[3..11].copy_from_slice(b"NTFS    ");
        let r = NtfsProbe.probe(&MemoryReader::new(data)).unwrap();
        assert!(!r.is_positive());
        assert!(r.evidence.iter().any(|e| !e.supports));
        assert!(
            !NtfsProbe
                .probe(&MemoryReader::zeroed(4096))
                .unwrap()
                .is_positive()
        );
    }
}
