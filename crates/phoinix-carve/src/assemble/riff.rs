//! RIFF containers (WAV, AVI, WebP): the declared chunk size.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len, le32};
use crate::CarveError;
use crate::probe::Probe;

/// RIFF assembler.
pub struct RiffAssembler;

impl Assembler for RiffAssembler {
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError> {
        let head = probe.read_available(start, 16)?;
        if head.get(..4) != Some(b"RIFF") || head.len() < 16 {
            return Ok(None);
        }
        let (Some(size), Some(form), Some(first_chunk)) =
            (le32(&head, 4), head.get(8..12), head.get(12..16))
        else {
            return Ok(None);
        };
        let size = u64::from(size).saturating_add(8);
        if !form.iter().all(|b| b.is_ascii_graphic() || *b == b' ')
            || !first_chunk
                .iter()
                .all(|b| b.is_ascii_graphic() || *b == b' ')
        {
            return Ok(None);
        }
        let (id, name, ext) = match form {
            b"WAVE" => ("wav", "WAVE audio", "wav"),
            b"AVI " => ("avi", "AVI video", "avi"),
            b"WEBP" => ("webp", "WebP image", "webp"),
            _ => ("riff", "RIFF container", "riff"),
        };
        let mut checks = vec![
            ValidationCheck::pass(
                "header",
                format!("RIFF form {}", String::from_utf8_lossy(form)),
            ),
            ValidationCheck::pass(
                "first chunk",
                String::from_utf8_lossy(first_chunk).into_owned(),
            ),
        ];
        let length = clamp_len(start, size, max_len, probe.limit());
        let mut a = if length < size {
            checks.push(ValidationCheck::fail(
                "declared size",
                format!("{size} bytes declared, only {length} readable"),
            ));
            Assembly::from_checks(length, false, checks).with_status(ValidationStatus::Damaged)
        } else {
            checks.push(ValidationCheck::pass(
                "declared size",
                format!("{size} bytes"),
            ));
            Assembly::from_checks(length, true, checks)
        };
        a.type_id = Some(id.into());
        a.type_name = Some(name.into());
        a.extension = Some(ext.into());
        Ok(Some(a))
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

    pub fn sample_wav(samples: usize) -> Vec<u8> {
        let data_len = samples * 2;
        let mut v = b"RIFF".to_vec();
        v.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&[1, 0, 1, 0]);
        v.extend_from_slice(&8000u32.to_le_bytes());
        v.extend_from_slice(&16000u32.to_le_bytes());
        v.extend_from_slice(&[2, 0, 16, 0]);
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data_len as u32).to_le_bytes());
        v.extend((0..data_len).map(|i| (i % 200) as u8));
        v
    }

    #[test]
    fn declared_size_and_form() {
        let wav = sample_wav(1000);
        let r = run(&RiffAssembler, &wav, b"zz").unwrap();
        assert_eq!(r.length, wav.len() as u64);
        assert_eq!(r.extension.as_deref(), Some("wav"));
        assert_eq!(r.status, ValidationStatus::Valid);
        assert!(run(&RiffAssembler, b"RIFF\x10\0\0\0\x01\x02\x03\x04abcd", b"").is_none());
    }
}
