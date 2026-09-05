//! GIF: header, screen descriptor, colour tables and block walk to the
//! trailer.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len, tolerate_truncation};
use crate::CarveError;
use crate::probe::Probe;

const MAX_BLOCKS: usize = 10_000_000;

/// GIF assembler.
pub struct GifAssembler;

impl Assembler for GifAssembler {
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

/// Skips data sub-blocks starting at `pos`; returns the position after the
/// terminator.
fn skip_sub_blocks(
    probe: &mut Probe<'_>,
    mut pos: u64,
    end: u64,
) -> Result<Option<u64>, CarveError> {
    let mut n = 0usize;
    while pos < end && n < MAX_BLOCKS {
        n += 1;
        let len = u64::from(probe.byte(pos)?);
        pos = pos.saturating_add(1);
        if len == 0 {
            return Ok(Some(pos));
        }
        pos = pos.saturating_add(len);
    }
    Ok(None)
}

fn walk(
    probe: &mut Probe<'_>,
    start: u64,
    max_len: u64,
    checks: &mut Vec<ValidationCheck>,
) -> Result<Option<Assembly>, CarveError> {
    let end = start.saturating_add(clamp_len(start, max_len, max_len, probe.limit()));
    let head = probe.read(start, 13)?;
    if head.get(..3) != Some(b"GIF") || !matches!(head.get(3..6), Some(b"87a" | b"89a")) {
        return Ok(None);
    }
    checks.push(ValidationCheck::pass(
        "header",
        String::from_utf8_lossy(head.get(..6).unwrap_or(b"GIF")).into_owned(),
    ));
    let w = u16::from_le_bytes([
        head.get(6).copied().unwrap_or(0),
        head.get(7).copied().unwrap_or(0),
    ]);
    let h = u16::from_le_bytes([
        head.get(8).copied().unwrap_or(0),
        head.get(9).copied().unwrap_or(0),
    ]);
    if w == 0 || h == 0 {
        checks.push(ValidationCheck::fail("screen", "zero-sized logical screen"));
        return Ok(Some(
            Assembly::from_checks(13, false, std::mem::take(checks))
                .with_status(ValidationStatus::Invalid),
        ));
    }
    checks.push(ValidationCheck::pass("screen", format!("{w}×{h}")));
    let flags = head.get(10).copied().unwrap_or(0);
    let mut pos = start.saturating_add(13);
    if flags & 0x80 != 0 {
        pos = pos.saturating_add(3u64 << ((flags & 7) + 1));
    }
    let mut images = 0u32;
    let mut blocks = 0usize;
    while pos < end && blocks < MAX_BLOCKS {
        blocks += 1;
        match probe.byte(pos)? {
            0x3B => {
                let length = pos.saturating_add(1) - start;
                checks.push(ValidationCheck::pass(
                    "blocks",
                    format!("{blocks} blocks, {images} image(s)"),
                ));
                checks.push(ValidationCheck::pass("trailer", "trailer found"));
                let status = if images == 0 {
                    ValidationStatus::MostlyValid
                } else {
                    ValidationStatus::Valid
                };
                return Ok(Some(
                    Assembly::from_checks(length, true, std::mem::take(checks)).with_status(status),
                ));
            }
            0x21 => {
                // Extension: label byte, then sub-blocks.
                let Some(next) = skip_sub_blocks(probe, pos.saturating_add(2), end)? else {
                    break;
                };
                pos = next;
            }
            0x2C => {
                images = images.saturating_add(1);
                let desc = probe.read(pos, 10)?;
                let lflags = desc.get(9).copied().unwrap_or(0);
                let mut next = pos.saturating_add(10);
                if lflags & 0x80 != 0 {
                    next = next.saturating_add(3u64 << ((lflags & 7) + 1));
                }
                // LZW minimum code size, then sub-blocks.
                let Some(after) = skip_sub_blocks(probe, next.saturating_add(1), end)? else {
                    break;
                };
                pos = after;
            }
            other => {
                checks.push(ValidationCheck::fail(
                    "blocks",
                    format!(
                        "unexpected block introducer {other:02X} at {} bytes in",
                        pos - start
                    ),
                ));
                return Ok(Some(
                    Assembly::from_checks(pos - start, false, std::mem::take(checks))
                        .with_status(ValidationStatus::Damaged),
                ));
            }
        }
    }
    checks.push(ValidationCheck::fail(
        "trailer",
        "trailer not found within the size limit",
    ));
    Ok(Some(
        Assembly::from_checks(pos.min(end) - start, false, std::mem::take(checks))
            .with_status(ValidationStatus::Damaged),
    ))
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

    pub fn sample_gif() -> Vec<u8> {
        let mut v = b"GIF89a".to_vec();
        v.extend_from_slice(&[4, 0, 2, 0, 0x80, 0, 0]); // 4x2, GCT of 2 entries
        v.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
        v.extend_from_slice(&[0x21, 0xF9, 4, 0, 0, 0, 0, 0]); // GCE
        v.extend_from_slice(&[0x2C, 0, 0, 0, 0, 4, 0, 2, 0, 0]); // image descriptor
        v.push(2); // LZW min code size
        v.extend_from_slice(&[3, 0x44, 0x01, 0x05, 0]); // one sub-block + terminator
        v.push(0x3B);
        v
    }

    #[test]
    fn walks_to_trailer() {
        let gif = sample_gif();
        let r = run(&GifAssembler, &gif, b"xx").unwrap();
        assert_eq!(r.length, gif.len() as u64);
        assert_eq!(r.status, ValidationStatus::Valid, "{:?}", r.checks);
        let r = run(&GifAssembler, &gif[..gif.len() - 1], &[0x99; 8]).unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(!r.end_known);
        assert!(run(&GifAssembler, b"GIF8xa..........", b"").is_none());
    }
}
