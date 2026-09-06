//! JPEG: marker walk, then the entropy-coded data up to the EOI marker.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len, tolerate_truncation};
use crate::CarveError;
use crate::probe::{Find, Probe, WINDOW_BYTES};

/// Segments walked before giving up on a runaway file.
const MAX_SEGMENTS: usize = 65_536;

/// JPEG assembler.
pub struct JpegAssembler;

fn is_sof(marker: u8) -> bool {
    matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF)
}

/// Markers that may legitimately interrupt entropy-coded data.
fn may_follow_scan(marker: u8) -> bool {
    matches!(marker, 0xC4 | 0xDA | 0xDD | 0xD9 | 0xDC | 0xC0..=0xCF | 0xE0..=0xEF | 0xFE)
}

impl Assembler for JpegAssembler {
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
    if probe.read(start, 2)? != [0xFF, 0xD8] {
        return Ok(None);
    }
    checks.push(ValidationCheck::pass(
        "SOI",
        "start-of-image marker present",
    ));
    let mut pos = start.saturating_add(2);
    let mut segments = 0usize;
    let mut dimensions: Option<(u16, u16)> = None;
    let mut in_scan = false;
    let mut scans = 0u32;
    while pos.saturating_add(2) <= end && segments < MAX_SEGMENTS {
        segments += 1;
        if in_scan {
            // Scan the entropy-coded data for the next marker. Real entropy
            // data carries a stuffed FF every few hundred bytes; a whole
            // window without one is not JPEG data any more (overwritten,
            // discarded or wiped), so the image ends there.
            let ff = match probe.find_bounded(&[0xFF], pos, end, &|w| !w.contains(&0xFF))? {
                Find::Found(ff) => ff,
                Find::GaveUp(at) => {
                    push_summary(checks, segments, dimensions, scans);
                    checks.push(ValidationCheck::fail(
                        "entropy data",
                        format!(
                            "no marker byte in the {WINDOW_BYTES} bytes after offset {}: the image data ends here (overwritten or discarded)",
                            at - start
                        ),
                    ));
                    return Ok(Some(
                        Assembly::from_checks(at - start, false, std::mem::take(checks))
                            .with_status(ValidationStatus::Damaged),
                    ));
                }
                Find::Exhausted => {
                    pos = end;
                    break;
                }
            };
            if ff.saturating_add(1) >= end {
                pos = end;
                break;
            }
            let next = probe.byte(ff.saturating_add(1))?;
            match next {
                0x00 | 0xD0..=0xD7 | 0xFF => {
                    // Stuffed byte, restart marker or fill: still scan data.
                    pos = ff.saturating_add(if next == 0xFF { 1 } else { 2 });
                    continue;
                }
                0xD9 => {
                    let length = ff.saturating_add(2) - start;
                    push_summary(checks, segments, dimensions, scans);
                    checks.push(ValidationCheck::pass("EOI", "end-of-image marker found"));
                    return Ok(Some(Assembly::from_checks(
                        length,
                        true,
                        std::mem::take(checks),
                    )));
                }
                m if may_follow_scan(m) => {
                    in_scan = false;
                    pos = ff;
                    continue;
                }
                _ => {
                    push_summary(checks, segments, dimensions, scans);
                    checks.push(ValidationCheck::fail(
                        "entropy data",
                        format!("invalid marker FF{next:02X} inside the scan data: the file is truncated or fragmented here"),
                    ));
                    let length = ff - start;
                    return Ok(Some(
                        Assembly::from_checks(length, false, std::mem::take(checks))
                            .with_status(ValidationStatus::Damaged),
                    ));
                }
            }
        }
        let head = probe.read(pos, 2)?;
        if head.first() != Some(&0xFF) {
            push_summary(checks, segments, dimensions, scans);
            checks.push(ValidationCheck::fail(
                "segment sequence",
                format!(
                    "expected a marker at {} bytes, found {:02X}",
                    pos - start,
                    head.first().copied().unwrap_or(0)
                ),
            ));
            return Ok(Some(
                Assembly::from_checks(pos - start, false, std::mem::take(checks)).with_status(
                    if segments <= 2 {
                        ValidationStatus::Invalid
                    } else {
                        ValidationStatus::Damaged
                    },
                ),
            ));
        }
        let marker = head.get(1).copied().unwrap_or(0);
        match marker {
            0xFF => {
                pos = pos.saturating_add(1);
            }
            0xD8 | 0x01 | 0xD0..=0xD7 => {
                pos = pos.saturating_add(2);
            }
            0xD9 => {
                let length = pos.saturating_add(2) - start;
                push_summary(checks, segments, dimensions, scans);
                checks.push(ValidationCheck::pass("EOI", "end-of-image marker found"));
                let status = if scans == 0 {
                    ValidationStatus::MostlyValid
                } else {
                    ValidationStatus::Valid
                };
                return Ok(Some(
                    Assembly::from_checks(length, true, std::mem::take(checks)).with_status(status),
                ));
            }
            _ => {
                let len = u64::from(probe.u16_be(pos.saturating_add(2))?);
                if len < 2 {
                    push_summary(checks, segments, dimensions, scans);
                    checks.push(ValidationCheck::fail(
                        "segment sequence",
                        format!("segment FF{marker:02X} has an invalid length"),
                    ));
                    return Ok(Some(
                        Assembly::from_checks(pos - start, false, std::mem::take(checks))
                            .with_status(ValidationStatus::Damaged),
                    ));
                }
                if is_sof(marker) && dimensions.is_none() {
                    let h = probe.u16_be(pos.saturating_add(5))?;
                    let w = probe.u16_be(pos.saturating_add(7))?;
                    dimensions = Some((w, h));
                }
                pos = pos.saturating_add(2).saturating_add(len);
                if marker == 0xDA {
                    in_scan = true;
                    scans = scans.saturating_add(1);
                }
            }
        }
    }
    push_summary(checks, segments, dimensions, scans);
    checks.push(ValidationCheck::fail(
        "EOI",
        "end-of-image marker not found within the size limit",
    ));
    Ok(Some(
        Assembly::from_checks(pos.min(end) - start, false, std::mem::take(checks))
            .with_status(ValidationStatus::Damaged),
    ))
}

fn push_summary(
    checks: &mut Vec<ValidationCheck>,
    segments: usize,
    dimensions: Option<(u16, u16)>,
    scans: u32,
) {
    checks.push(ValidationCheck::pass(
        "segment sequence",
        format!("{segments} segments walked"),
    ));
    match dimensions {
        Some((w, h)) if w > 0 && h > 0 => {
            checks.push(ValidationCheck::pass("dimensions", format!("{w}×{h}")));
        }
        Some(_) => checks.push(ValidationCheck::fail("dimensions", "zero-sized frame")),
        None => checks.push(ValidationCheck::fail(
            "dimensions",
            "no frame header (SOF) seen",
        )),
    }
    if scans > 0 {
        checks.push(ValidationCheck::pass(
            "scan data",
            format!("{scans} scan(s)"),
        ));
    } else {
        checks.push(ValidationCheck::fail(
            "scan data",
            "no start-of-scan segment",
        ));
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

    /// A structurally valid baseline JPEG (not decodable, but every marker
    /// is where a decoder expects it).
    #[test]
    fn entropy_scan_gives_up_when_the_data_stops_being_jpeg() {
        // A JPEG cut before its EOI marker, followed by megabytes of zeros:
        // the walk must end near the cut, not at the size limit.
        let entropy: Vec<u8> = (0..3000u32).map(|i| (i % 253) as u8).collect();
        let jpeg = sample_jpeg(&entropy);
        let cut = &jpeg[..jpeg.len() - 2];
        let zeros = vec![0u8; 3 * WINDOW_BYTES];
        let r = run(&JpegAssembler, cut, &zeros).unwrap();
        assert!(!r.end_known);
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(r.length < 2 * WINDOW_BYTES as u64, "{}", r.length);
    }

    pub fn sample_jpeg(entropy: &[u8]) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        // APP0 JFIF
        v.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x10]);
        v.extend_from_slice(b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00");
        // DQT (64 entries + id)
        v.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
        v.extend_from_slice(&[1u8; 64]);
        // SOF0: 8-bit, 16x8, 1 component
        v.extend_from_slice(&[
            0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x08, 0x00, 0x10, 0x01, 0x01, 0x11, 0x00,
        ]);
        // DHT minimal
        v.extend_from_slice(&[0xFF, 0xC4, 0x00, 0x14, 0x00]);
        v.extend_from_slice(&[0u8; 16]);
        v.push(0x00);
        // SOS
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        v.extend_from_slice(entropy);
        v.extend_from_slice(&[0xFF, 0xD9]);
        v
    }

    #[test]
    fn walks_to_eoi_and_reports_damage() {
        let entropy: Vec<u8> = (0..500u32)
            .map(|i| (i % 251) as u8)
            .chain([0xFF, 0x00, 0xFF, 0xD0, 0x12])
            .collect();
        let jpeg = sample_jpeg(&entropy);
        let r = run(&JpegAssembler, &jpeg, b"garbage after").unwrap();
        assert_eq!(r.length, jpeg.len() as u64);
        assert!(r.end_known);
        assert_eq!(r.status, ValidationStatus::Valid, "{:?}", r.checks);
        assert!(
            r.checks
                .iter()
                .any(|c| c.name == "dimensions" && c.detail == "16×8")
        );
        // Truncated: an invalid marker appears where foreign data begins.
        let cut = &jpeg[..jpeg.len() - 100];
        let mut tail = vec![0xFF, 0x13];
        tail.extend_from_slice(&[7u8; 64]);
        let r = run(&JpegAssembler, cut, &tail).unwrap();
        assert!(!r.end_known);
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert_eq!(r.length, cut.len() as u64);
        // Not a JPEG at all.
        assert!(run(&JpegAssembler, b"FFD8 nope", b"").is_none());
        // Bad segment right after SOI: invalid.
        let r = run(&JpegAssembler, &[0xFF, 0xD8, 0x12, 0x34], b"").unwrap();
        assert_eq!(r.status, ValidationStatus::Invalid);
        // No EOI within the region: damaged, open-ended.
        let r = run(&JpegAssembler, &jpeg[..jpeg.len() - 2], b"").unwrap();
        assert!(!r.end_known);
    }
}
