//! ISO base media files (MP4, MOV, M4A, HEIC): top-level box walk.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len, tolerate_truncation};
use crate::CarveError;
use crate::probe::Probe;

const MAX_BOXES: usize = 100_000;

const KNOWN: &[&[u8; 4]] = &[
    b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"meta", b"uuid", b"moof", b"mfra",
    b"sidx", b"styp", b"pdin", b"junk", b"pnot", b"PICT", b"udta", b"ssix", b"prft", b"emsg",
    b"mfhd", b"iods", b"beam",
];

/// ISO media assembler.
pub struct Mp4Assembler;

impl Assembler for Mp4Assembler {
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

fn brand_extension(brand: &[u8]) -> (&'static str, &'static str, &'static str) {
    match brand {
        b"qt  " => ("mov", "QuickTime movie", "mov"),
        b"M4A " => ("m4a", "MPEG-4 audio", "m4a"),
        b"heic" | b"heix" | b"mif1" | b"msf1" => ("heic", "HEIF image", "heic"),
        b"avif" | b"avis" => ("avif", "AVIF image", "avif"),
        b"3gp4" | b"3gp5" | b"3gp6" | b"3gg6" => ("3gp", "3GPP video", "3gp"),
        _ => ("mp4", "MPEG-4 video", "mp4"),
    }
}

fn walk(
    probe: &mut Probe<'_>,
    start: u64,
    max_len: u64,
    checks: &mut Vec<ValidationCheck>,
) -> Result<Option<Assembly>, CarveError> {
    let end = start.saturating_add(clamp_len(start, max_len, max_len, probe.limit()));
    let head = probe.read(start, 16)?;
    if head.get(4..8) != Some(b"ftyp") {
        return Ok(None);
    }
    let brand = head.get(8..12).unwrap_or(b"    ");
    if !brand.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        return Ok(None);
    }
    let (id, name, ext) = brand_extension(brand);
    checks.push(ValidationCheck::pass(
        "ftyp",
        format!("brand {}", String::from_utf8_lossy(brand)),
    ));
    let mut pos = start;
    let mut boxes = 0usize;
    let mut seen_moov = false;
    let mut seen_mdat = false;
    let finish = |length: u64,
                  end_known: bool,
                  checks: &mut Vec<ValidationCheck>,
                  boxes: usize,
                  moov: bool,
                  mdat: bool| {
        checks.push(ValidationCheck::pass(
            "boxes",
            format!("{boxes} top-level box(es)"),
        ));
        if moov {
            checks.push(ValidationCheck::pass("moov", "movie header present"));
        } else {
            checks.push(ValidationCheck::fail("moov", "no movie header (moov) box"));
        }
        if mdat {
            checks.push(ValidationCheck::pass("mdat", "media data present"));
        } else {
            checks.push(ValidationCheck::fail("mdat", "no media data (mdat) box"));
        }
        let status = if !end_known {
            ValidationStatus::Damaged
        } else if moov && mdat {
            ValidationStatus::Valid
        } else {
            ValidationStatus::MostlyValid
        };
        let mut a =
            Assembly::from_checks(length, end_known, std::mem::take(checks)).with_status(status);
        a.type_id = Some(id.into());
        a.type_name = Some(name.into());
        a.extension = Some(ext.into());
        a
    };
    while pos.saturating_add(8) <= end && boxes < MAX_BOXES {
        let size32 = probe.u32_be(pos)?;
        let kind = probe.read(pos.saturating_add(4), 4)?;
        let known = KNOWN.iter().any(|k| kind == *k);
        if !known {
            // The file ends where the box sequence stops making sense.
            return Ok(Some(finish(
                pos - start,
                true,
                checks,
                boxes,
                seen_moov,
                seen_mdat,
            )));
        }
        boxes += 1;
        if kind == b"moov" {
            seen_moov = true;
        }
        if kind == b"mdat" {
            seen_mdat = true;
        }
        let size = match size32 {
            0 => {
                // Extends to the end of the file: unknowable here.
                checks.push(ValidationCheck::fail(
                    "box sizes",
                    "a box declares size 0 (to end of file); the end cannot be determined",
                ));
                return Ok(Some(finish(
                    end - start,
                    false,
                    checks,
                    boxes,
                    seen_moov,
                    seen_mdat,
                )));
            }
            1 => probe.u64_be(pos.saturating_add(8))?,
            n => u64::from(n),
        };
        if size < 8 {
            checks.push(ValidationCheck::fail(
                "box sizes",
                format!(
                    "box {} declares an invalid size",
                    String::from_utf8_lossy(&kind)
                ),
            ));
            return Ok(Some(finish(
                pos - start,
                false,
                checks,
                boxes,
                seen_moov,
                seen_mdat,
            )));
        }
        let next = pos.saturating_add(size);
        if next > end {
            checks.push(ValidationCheck::fail(
                "box sizes",
                format!(
                    "box {} runs past the size limit",
                    String::from_utf8_lossy(&kind)
                ),
            ));
            return Ok(Some(finish(
                end - start,
                false,
                checks,
                boxes,
                seen_moov,
                seen_mdat,
            )));
        }
        pos = next;
    }
    Ok(Some(finish(
        pos - start,
        true,
        checks,
        boxes,
        seen_moov,
        seen_mdat,
    )))
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

    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(payload);
        v
    }

    pub fn sample_mp4() -> Vec<u8> {
        let mut v = boxed(b"ftyp", b"isomisomiso2mp41");
        v.extend(boxed(b"moov", &[0u8; 100]));
        v.extend(boxed(b"mdat", &[0x5Au8; 3000]));
        v
    }

    #[test]
    fn walks_boxes() {
        let mp4 = sample_mp4();
        let r = run(&Mp4Assembler, &mp4, b"not a box").unwrap();
        assert_eq!(r.length, mp4.len() as u64, "{:?}", r.checks);
        assert_eq!(r.status, ValidationStatus::Valid);
        assert_eq!(r.extension.as_deref(), Some("mp4"));
        let r = run(&Mp4Assembler, &mp4[..mp4.len() - 100], b"").unwrap();
        assert!(!r.end_known);
        let mut mov = boxed(b"ftyp", b"qt  \0\0\0\0qt  ");
        mov.extend(boxed(b"moov", &[0u8; 10]));
        let r = run(&Mp4Assembler, &mov, b"").unwrap();
        assert_eq!(r.extension.as_deref(), Some("mov"));
        assert_eq!(r.status, ValidationStatus::MostlyValid);
    }
}
