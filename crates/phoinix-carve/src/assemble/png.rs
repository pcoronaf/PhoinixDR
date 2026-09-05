//! PNG: chunk walk with CRC verification up to IEND.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len, tolerate_truncation};
use crate::CarveError;
use crate::probe::Probe;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const MAX_CHUNKS: usize = 1_000_000;
/// Chunks larger than this are not CRC-checked byte by byte (cost bound).
const CRC_CHECK_LIMIT: u64 = 64 * 1024 * 1024;

/// PNG assembler.
pub struct PngAssembler;

impl Assembler for PngAssembler {
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError> {
        let mut checks = Vec::new();
        let result = walk(probe, start, max_len, &mut checks);
        tolerate_truncation(result, start, probe.limit(), checks)
    }
}

fn walk(
    probe: &mut Probe<'_>,
    start: u64,
    max_len: u64,
    checks: &mut Vec<ValidationCheck>,
) -> Result<Option<Assembly>, CarveError> {
    let end = start.saturating_add(clamp_len(start, max_len, max_len, probe.limit()));
    if probe.read(start, 8)? != SIGNATURE {
        return Ok(None);
    }
    checks.push(ValidationCheck::pass("signature", "PNG signature present"));
    let mut pos = start.saturating_add(8);
    let mut chunks = 0usize;
    let mut crc_failures = 0u32;
    let mut ihdr_ok = false;
    while pos.saturating_add(12) <= end && chunks < MAX_CHUNKS {
        chunks += 1;
        let len = u64::from(probe.u32_be(pos)?);
        let kind = probe.read(pos.saturating_add(4), 4)?;
        if !kind.iter().all(u8::is_ascii_alphabetic) {
            checks.push(ValidationCheck::fail(
                "chunk sequence",
                format!(
                    "chunk {chunks} has a non-ASCII type at {} bytes in",
                    pos - start
                ),
            ));
            return Ok(Some(damaged(pos - start, checks, chunks == 1)));
        }
        let data_start = pos.saturating_add(8);
        let crc_pos = data_start.saturating_add(len);
        if crc_pos.saturating_add(4) > end {
            checks.push(ValidationCheck::fail(
                "chunk sequence",
                format!(
                    "chunk {} ({} bytes) runs past the size limit",
                    String::from_utf8_lossy(&kind),
                    len
                ),
            ));
            return Ok(Some(damaged(pos - start, checks, false)));
        }
        if chunks == 1 {
            if kind != b"IHDR" || len != 13 {
                checks.push(ValidationCheck::fail(
                    "IHDR",
                    "first chunk is not a 13-byte IHDR",
                ));
                return Ok(Some(damaged(pos - start, checks, true)));
            }
            let w = probe.u32_be(data_start)?;
            let h = probe.u32_be(data_start.saturating_add(4))?;
            if w == 0 || h == 0 {
                checks.push(ValidationCheck::fail("IHDR", "zero-sized image"));
            } else {
                checks.push(ValidationCheck::pass("IHDR", format!("{w}×{h}")));
                ihdr_ok = true;
            }
        }
        if len <= CRC_CHECK_LIMIT {
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&kind);
            let mut off = data_start;
            let mut remaining = len;
            while remaining > 0 {
                let take = usize::try_from(remaining.min(1 << 20)).unwrap_or(1 << 20);
                let piece = probe.read(off, take)?;
                hasher.update(&piece);
                off = off.saturating_add(take as u64);
                remaining -= take as u64;
            }
            let stored = probe.u32_be(crc_pos)?;
            if hasher.finalize() != stored {
                crc_failures += 1;
                if crc_failures == 1 {
                    checks.push(ValidationCheck::fail(
                        "chunk CRC",
                        format!("CRC mismatch in chunk {} ({}) at {} bytes in: the content is damaged or fragmented here", chunks, String::from_utf8_lossy(&kind), pos - start),
                    ));
                }
            }
        }
        pos = crc_pos.saturating_add(4);
        if kind == b"IEND" {
            checks.push(ValidationCheck::pass(
                "chunk sequence",
                format!("{chunks} chunks walked"),
            ));
            if crc_failures == 0 {
                checks.push(ValidationCheck::pass(
                    "chunk CRC",
                    "every chunk CRC matches",
                ));
            }
            checks.push(ValidationCheck::pass("IEND", "end chunk found"));
            let status = if crc_failures > 0 || !ihdr_ok {
                ValidationStatus::Damaged
            } else {
                ValidationStatus::Valid
            };
            return Ok(Some(
                Assembly::from_checks(pos - start, true, std::mem::take(checks))
                    .with_status(status),
            ));
        }
    }
    checks.push(ValidationCheck::fail(
        "IEND",
        "end chunk not found within the size limit",
    ));
    Ok(Some(damaged(pos.min(end) - start, checks, false)))
}

fn damaged(length: u64, checks: &mut Vec<ValidationCheck>, invalid: bool) -> Assembly {
    Assembly::from_checks(length, false, std::mem::take(checks)).with_status(if invalid {
        ValidationStatus::Invalid
    } else {
        ValidationStatus::Damaged
    })
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

    pub fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        let mut h = crc32fast::Hasher::new();
        h.update(kind);
        h.update(data);
        v.extend_from_slice(&h.finalize().to_be_bytes());
        v
    }

    pub fn sample_png(payload: &[u8]) -> Vec<u8> {
        let mut v = SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&20u32.to_be_bytes());
        ihdr.extend_from_slice(&10u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        v.extend(chunk(b"IHDR", &ihdr));
        v.extend(chunk(b"IDAT", payload));
        v.extend(chunk(b"IEND", &[]));
        v
    }

    #[test]
    fn walks_chunks_and_detects_crc_damage() {
        let png = sample_png(&[5u8; 3000]);
        let r = run(&PngAssembler, &png, b"tail").unwrap();
        assert_eq!(r.length, png.len() as u64);
        assert_eq!(r.status, ValidationStatus::Valid, "{:?}", r.checks);
        let mut broken = png.clone();
        broken[100] ^= 0xFF;
        let r = run(&PngAssembler, &broken, b"").unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(r.end_known);
        let r = run(&PngAssembler, &png[..png.len() - 5], b"").unwrap();
        assert!(!r.end_known);
        assert!(run(&PngAssembler, b"not png", b"").is_none());
        let mut bad = SIGNATURE.to_vec();
        bad.extend_from_slice(&[0, 0, 0, 4, 0xFF, 0, 0, 0]);
        let r = run(&PngAssembler, &bad, &[0u8; 32]).unwrap();
        assert_eq!(r.status, ValidationStatus::Invalid);
    }
}
