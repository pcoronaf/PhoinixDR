//! PDF: the linearization dictionary declares the length; otherwise the
//! `%%EOF` markers of the incremental-update chain are followed.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len};
use crate::CarveError;
use crate::probe::{Probe, find_in};

const MAX_UPDATES: usize = 4096;

/// PDF assembler.
pub struct PdfAssembler;

/// Parses `/L <digits>` from the linearization dictionary.
fn linearized_length(head: &[u8]) -> Option<u64> {
    let lin = find_in(head, b"/Linearized")?;
    let rest = head.get(lin..)?;
    let l = find_in(rest, b"/L ")? + 3;
    let digits: String = rest
        .get(l..)?
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .map(|b| char::from(*b))
        .collect();
    digits.parse().ok()
}

/// Whether the bytes after an `%%EOF` look like an incremental update
/// (another body/xref/trailer) rather than foreign data.
fn continues_after_eof(next: &[u8]) -> bool {
    let trimmed: Vec<u8> = next
        .iter()
        .copied()
        .skip_while(u8::is_ascii_whitespace)
        .collect();
    if trimmed.starts_with(b"%PDF") {
        return false;
    }
    if trimmed.starts_with(b"xref")
        || trimmed.starts_with(b"trailer")
        || trimmed.starts_with(b"startxref")
        || trimmed.starts_with(b"%")
    {
        return true;
    }
    // "<num> <gen> obj"
    let digits = trimmed.iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        return false;
    }
    let after = trimmed.get(digits..).unwrap_or(&[]);
    let after: Vec<u8> = after.iter().copied().skip_while(|b| *b == b' ').collect();
    let generation = after.iter().take_while(|b| b.is_ascii_digit()).count();
    generation > 0
        && after
            .get(generation..)
            .map(|a| {
                a.iter()
                    .copied()
                    .skip_while(|b| *b == b' ')
                    .take(3)
                    .collect::<Vec<u8>>()
            })
            .is_some_and(|w| w == b"obj")
}

impl Assembler for PdfAssembler {
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError> {
        let bound = clamp_len(start, max_len, max_len, probe.limit());
        let end = start.saturating_add(bound);
        let head = probe.read_available(start, 1024)?;
        if head.get(..5) != Some(b"%PDF-") {
            return Ok(None);
        }
        let version = head
            .get(5..8)
            .map(|v| String::from_utf8_lossy(v).into_owned())
            .unwrap_or_default();
        let mut checks = vec![ValidationCheck::pass("header", format!("%PDF-{version}"))];

        if let Some(len) = linearized_length(&head)
            && len >= 32
            && len <= bound
        {
            let tail = probe.read(start.saturating_add(len.saturating_sub(64)), 64)?;
            if find_in(&tail, b"%%EOF").is_some() {
                checks.push(ValidationCheck::pass(
                    "linearization",
                    format!("/L declares {len} bytes and %%EOF sits there"),
                ));
                return Ok(Some(Assembly::from_checks(len, true, checks)));
            }
            checks.push(ValidationCheck::fail(
                "linearization",
                format!("/L declares {len} bytes but no %%EOF is there"),
            ));
        }

        let mut pos = start.saturating_add(8);
        let mut last_end: Option<u64> = None;
        let mut updates = 0usize;
        while updates < MAX_UPDATES {
            updates += 1;
            let Some(eof) = probe.find(b"%%EOF", pos, end)? else {
                break;
            };
            let mut file_end = eof.saturating_add(5);
            // Consume the line ending.
            for _ in 0..2 {
                if file_end < end && matches!(probe.byte(file_end)?, b'\r' | b'\n') {
                    file_end = file_end.saturating_add(1);
                } else {
                    break;
                }
            }
            last_end = Some(file_end);
            let next = probe.read_available(file_end, 64)?;
            if next.is_empty() || !continues_after_eof(&next) {
                break;
            }
            pos = file_end;
        }
        match last_end {
            Some(file_end) => {
                let length = file_end - start;
                let tail_from = file_end.saturating_sub(2048).max(start);
                let tail = probe.read(
                    tail_from,
                    usize::try_from(file_end - tail_from).unwrap_or(2048),
                )?;
                if find_in(&tail, b"startxref").is_some() {
                    checks.push(ValidationCheck::pass(
                        "trailer",
                        "startxref precedes the final %%EOF",
                    ));
                } else {
                    checks.push(ValidationCheck::fail(
                        "trailer",
                        "no startxref before the final %%EOF",
                    ));
                }
                checks.push(ValidationCheck::pass(
                    "%%EOF",
                    format!("{updates} revision(s), ends at {length} bytes"),
                ));
                Ok(Some(Assembly::from_checks(length, true, checks)))
            }
            None => {
                // No end marker: stop at the last "endobj" if any, else the bound.
                let scanned = probe.read_available(
                    start,
                    usize::try_from(bound.min(64 * 1024 * 1024)).unwrap_or(usize::MAX),
                )?;
                let last_obj = scanned
                    .windows(6)
                    .rposition(|w| w == b"endobj")
                    .map(|p| p + 6);
                let length = last_obj.map_or(bound, |p| p as u64);
                checks.push(ValidationCheck::fail("%%EOF", "no end-of-file marker within the size limit; the document is truncated or fragmented"));
                Ok(Some(
                    Assembly::from_checks(length, false, checks)
                        .with_status(ValidationStatus::Damaged),
                ))
            }
        }
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

    pub fn sample_pdf(body: &str) -> Vec<u8> {
        let mut s =
            String::from("%PDF-1.4\n%\u{e2}\u{e3}\n1 0 obj\n<< /Type /Catalog >>\nendobj\n");
        s.push_str(&format!(
            "2 0 obj\n<< /Length {} >>\nstream\n{}\nendstream\nendobj\n",
            body.len(),
            body
        ));
        let xref = s.len();
        s.push_str("xref\n0 3\n0000000000 65535 f \n0000000010 00000 n \n0000000050 00000 n \n");
        s.push_str(&format!(
            "trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        ));
        s.into_bytes()
    }

    #[test]
    fn follows_incremental_updates() {
        let base = sample_pdf("hello world");
        let mut updated = base.clone();
        updated.extend_from_slice(b"3 0 obj\n<< /Type /Annot >>\nendobj\nxref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 4 /Prev 100 >>\nstartxref\n300\n%%EOF\n");
        let r = run(&PdfAssembler, &updated, b"%PDF-1.7\nanother file").unwrap();
        assert_eq!(r.length, updated.len() as u64, "{:?}", r.checks);
        assert_eq!(r.status, ValidationStatus::Valid);
        let r = run(&PdfAssembler, &base, b"random tail data").unwrap();
        assert_eq!(r.length, base.len() as u64);
        // Truncated: no %%EOF.
        let cut = &base[..base.len() - 20];
        let r = run(&PdfAssembler, cut, b"zzz").unwrap();
        assert!(!r.end_known);
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(run(&PdfAssembler, b"%PDX-1.4", b"").is_none());
    }

    #[test]
    fn linearized_length_wins() {
        let mut pdf =
            b"%PDF-1.5\n%\xe2\xe3\n5 0 obj\n<< /Linearized 1 /L 200 /H [ 1 2 ] >>\nendobj\n"
                .to_vec();
        while pdf.len() < 195 {
            pdf.push(b' ');
        }
        pdf.extend_from_slice(b"%%EOF");
        assert_eq!(pdf.len(), 200);
        let r = run(&PdfAssembler, &pdf, b"garbage %%EOF later").unwrap();
        assert_eq!(r.length, 200);
        assert!(r.end_known);
        assert!(continues_after_eof(b"\n12 0 obj\n<<>>"));
        assert!(continues_after_eof(b"xref\n"));
        assert!(!continues_after_eof(b"\n\nhello"));
        assert!(!continues_after_eof(b"%PDF-1.4"));
    }
}
