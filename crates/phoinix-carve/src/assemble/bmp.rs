//! BMP: the file header declares the size.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len, le32};
use crate::CarveError;
use crate::probe::Probe;

/// BMP assembler.
pub struct BmpAssembler;

impl Assembler for BmpAssembler {
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError> {
        let head = probe.read_available(start, 26)?;
        if head.get(..2) != Some(b"BM") || head.len() < 26 {
            return Ok(None);
        }
        let (Some(size), Some(pixel_offset), Some(dib)) =
            (le32(&head, 2), le32(&head, 10), le32(&head, 14))
        else {
            return Ok(None);
        };
        let size = u64::from(size);
        let pixel_offset = u64::from(pixel_offset);
        let dib_ok = matches!(dib, 12 | 16 | 40 | 52 | 56 | 64 | 108 | 124);
        let reserved_ok = head.get(6..10).is_some_and(|r| r.iter().all(|b| *b == 0));
        if !dib_ok || size < 26 || pixel_offset >= size || pixel_offset < 14 + u64::from(dib) {
            return Ok(None);
        }
        let mut checks = vec![
            ValidationCheck::pass("header", "BM signature present"),
            ValidationCheck::pass("DIB header", format!("{dib}-byte header")),
        ];
        if reserved_ok {
            checks.push(ValidationCheck::pass(
                "reserved",
                "reserved fields are zero",
            ));
        } else {
            checks.push(ValidationCheck::fail(
                "reserved",
                "reserved fields are not zero",
            ));
        }
        let length = clamp_len(start, size, max_len, probe.limit());
        if length < size {
            checks.push(ValidationCheck::fail(
                "declared size",
                format!("{size} bytes declared, only {length} readable"),
            ));
            return Ok(Some(
                Assembly::from_checks(length, false, checks).with_status(ValidationStatus::Damaged),
            ));
        }
        checks.push(ValidationCheck::pass(
            "declared size",
            format!("{size} bytes"),
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

    pub fn sample_bmp() -> Vec<u8> {
        let pixels = 2 * 2 * 3 + 4 * 2; // 2x2, padded rows
        let size = 14 + 40 + pixels;
        let mut v = b"BM".to_vec();
        v.extend_from_slice(&(size as u32).to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&54u32.to_le_bytes());
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&2i32.to_le_bytes());
        v.extend_from_slice(&2i32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(&[0u8; 24]);
        v.extend(std::iter::repeat_n(0xAB, pixels));
        v
    }

    #[test]
    fn declared_size() {
        let bmp = sample_bmp();
        let r = run(&BmpAssembler, &bmp, b"zz").unwrap();
        assert_eq!(r.length, bmp.len() as u64);
        assert_eq!(r.status, ValidationStatus::Valid);
        assert!(run(&BmpAssembler, b"BMxxxxxxxxxxxxxxxxxxxxxxxxxxxx", b"").is_none());
        let r = run(&BmpAssembler, &bmp[..bmp.len() - 4], b"").unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
    }
}
