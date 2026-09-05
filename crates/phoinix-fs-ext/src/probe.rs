//! ext2/3/4 probe backed by the superblock parser.

use phoinix_block::{BlockReader, BlockReaderExt};
use phoinix_core::FileSystemType;
use phoinix_fs::{FileSystemProbe, FsError, ProbeEvidence, ProbeResult};

use crate::superblock::{SUPERBLOCK_OFFSET, Superblock};

/// Recognises ext2/3/4 volumes.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExtProbe;

impl FileSystemProbe for ExtProbe {
    fn filesystem(&self) -> FileSystemType {
        FileSystemType::Ext
    }

    fn probe(&self, reader: &dyn BlockReader) -> Result<ProbeResult, FsError> {
        if reader.len() < SUPERBLOCK_OFFSET + 1024 {
            return Ok(ProbeResult::negative(
                FileSystemType::Ext,
                "source shorter than a superblock",
            ));
        }
        let bytes = reader.read_vec(SUPERBLOCK_OFFSET, 1024)?;
        let sb = match Superblock::parse(&bytes) {
            Ok(sb) => sb,
            Err(e) => return Ok(ProbeResult::negative(FileSystemType::Ext, e.to_string())),
        };
        let mut evidence = vec![
            ProbeEvidence::supports("superblock magic 0xEF53 present"),
            ProbeEvidence::supports(format!(
                "{}: {} blocks of {} bytes, {} inodes",
                sb.flavour(),
                sb.blocks_count,
                sb.block_size,
                sb.inodes_count
            )),
        ];
        let mut confidence: u8 = 80;
        let fits = sb.blocks_count.saturating_mul(u64::from(sb.block_size))
            <= reader.len().saturating_add(u64::from(sb.block_size));
        if fits {
            evidence.push(ProbeEvidence::supports("declared size fits the source"));
            confidence += 5;
        } else {
            evidence.push(ProbeEvidence::contradicts(
                "declared size exceeds the source",
            ));
            confidence = confidence.saturating_sub(15);
        }
        match sb.checksum_ok {
            Some(true) => {
                evidence.push(ProbeEvidence::supports("superblock checksum matches"));
                confidence += 10;
            }
            Some(false) => {
                evidence.push(ProbeEvidence::contradicts(
                    "superblock checksum does not match",
                ));
                confidence = confidence.saturating_sub(20);
            }
            None => {}
        }
        if !sb.volume_name.is_empty() {
            evidence.push(ProbeEvidence::supports(format!(
                "volume label {:?}",
                sb.volume_name
            )));
        }
        Ok(ProbeResult {
            filesystem: FileSystemType::Ext,
            confidence: confidence.min(100),
            evidence,
        })
    }
}
