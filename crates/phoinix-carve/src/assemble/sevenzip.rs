//! 7-Zip: the start header locates the end header.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len, le32, le64};
use crate::CarveError;
use crate::probe::Probe;

const MAGIC: [u8; 6] = [0x37, 0x7A, 0xBC, 0xAF, 0x27, 0x1C];

/// 7-Zip assembler.
pub struct SevenZipAssembler;

impl Assembler for SevenZipAssembler {
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError> {
        let head = probe.read_available(start, 32)?;
        if head.get(..6) != Some(&MAGIC[..]) || head.len() < 32 {
            return Ok(None);
        }
        let (Some(stored_crc), Some(fields), Some(next_offset), Some(next_size)) = (
            le32(&head, 8),
            head.get(12..32),
            le64(&head, 12),
            le64(&head, 20),
        ) else {
            return Ok(None);
        };
        if stored_crc != crc32fast::hash(fields) {
            return Ok(None);
        }
        let mut checks = vec![
            ValidationCheck::pass(
                "signature",
                format!(
                    "7z version {}.{}",
                    head.get(6).copied().unwrap_or(0),
                    head.get(7).copied().unwrap_or(0)
                ),
            ),
            ValidationCheck::pass("start header CRC", "matches"),
        ];
        let size = 32u64.saturating_add(next_offset).saturating_add(next_size);
        let length = clamp_len(start, size, max_len, probe.limit());
        if length < size {
            checks.push(ValidationCheck::fail(
                "end header",
                format!("archive declares {size} bytes, only {length} readable"),
            ));
            return Ok(Some(
                Assembly::from_checks(length, false, checks).with_status(ValidationStatus::Damaged),
            ));
        }
        checks.push(ValidationCheck::pass(
            "end header",
            format!("{next_size} bytes at offset {}", 32 + next_offset),
        ));
        Ok(Some(Assembly::from_checks(length, true, checks)))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]
    use super::super::testutil::run;
    use super::*;

    pub fn sample_7z(packed: usize, header: usize) -> Vec<u8> {
        let mut v = MAGIC.to_vec();
        v.extend_from_slice(&[0, 4]);
        let mut fields = Vec::new();
        fields.extend_from_slice(&(packed as u64).to_le_bytes());
        fields.extend_from_slice(&(header as u64).to_le_bytes());
        fields.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        v.extend_from_slice(&crc32fast::hash(&fields).to_le_bytes());
        v.extend_from_slice(&fields);
        v.extend(std::iter::repeat_n(0x77, packed + header));
        v
    }

    #[test]
    fn start_header() {
        let z = sample_7z(500, 40);
        let r = run(&SevenZipAssembler, &z, b"tail").unwrap();
        assert_eq!(r.length, z.len() as u64);
        assert_eq!(r.status, ValidationStatus::Valid);
        let mut bad = z.clone();
        bad[9] ^= 1;
        assert!(run(&SevenZipAssembler, &bad, b"").is_none());
        let r = run(&SevenZipAssembler, &z[..300], b"").unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
    }
}
