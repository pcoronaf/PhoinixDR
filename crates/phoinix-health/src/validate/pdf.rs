//! PDF validator: header, `%%EOF`, `startxref`, cross-reference sanity.

use super::{FileValidator, ReadSeek, ValidationCheck, ValidationResult, read_at};

/// PDF structural validator.
#[derive(Debug, Default, Clone, Copy)]
pub struct PdfValidator;

fn find_last(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

impl FileValidator for PdfValidator {
    fn id(&self) -> &'static str {
        "pdf"
    }

    fn validate(
        &self,
        stream: &mut dyn ReadSeek,
        len: u64,
        _budget: u64,
    ) -> std::io::Result<ValidationResult> {
        let mut checks = Vec::new();
        let head = read_at(stream, 0, usize::try_from(len.min(16)).unwrap_or(0))?;
        let header_ok = head.starts_with(b"%PDF-1.") || head.starts_with(b"%PDF-2.");
        checks.push(if header_ok {
            ValidationCheck::pass(
                "PDF header",
                String::from_utf8_lossy(head.get(..8).unwrap_or(&head))
                    .trim()
                    .to_owned(),
            )
        } else {
            ValidationCheck::fail("PDF header", "missing %PDF- header")
        });
        if !header_ok {
            return Ok(ValidationResult::from_checks(checks));
        }

        let tail_len = len.min(2048);
        let tail = read_at(
            stream,
            len - tail_len,
            usize::try_from(tail_len).unwrap_or(0),
        )?;
        let eof = find_last(&tail, b"%%EOF");
        checks.push(match eof {
            Some(_) => ValidationCheck::pass("End-of-file marker", "%%EOF present near the end"),
            None => ValidationCheck::fail(
                "End-of-file marker",
                "%%EOF not found in the last 2 KiB; the tail is missing",
            ),
        });

        let startxref = find_last(&tail, b"startxref").and_then(|p| {
            let rest = tail.get(p + 9..)?;
            let text = String::from_utf8_lossy(rest);
            text.split_whitespace().next()?.parse::<u64>().ok()
        });
        match startxref {
            Some(offset) if offset < len => {
                let probe = read_at(
                    stream,
                    offset,
                    usize::try_from((len - offset).min(16)).unwrap_or(0),
                )?;
                let text = String::from_utf8_lossy(&probe);
                let ok = text.starts_with("xref") || text.split_whitespace().nth(2) == Some("obj");
                checks.push(if ok {
                    ValidationCheck::pass(
                        "Cross-reference",
                        format!("startxref {offset} points at a table or stream"),
                    )
                } else {
                    ValidationCheck::fail(
                        "Cross-reference",
                        format!("startxref {offset} does not point at xref data"),
                    )
                });
            }
            Some(offset) => checks.push(ValidationCheck::fail(
                "Cross-reference",
                format!("startxref {offset} lies beyond the file"),
            )),
            None => checks.push(ValidationCheck::fail(
                "Cross-reference",
                "startxref not found",
            )),
        }
        Ok(ValidationResult::from_checks(checks))
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builds a minimal but consistent PDF.

    #![allow(missing_docs)]

    pub fn build_pdf(payload_len: usize) -> Vec<u8> {
        let mut pdf = String::new();
        pdf.push_str("%PDF-1.4\n");
        let o1 = pdf.len();
        pdf.push_str("1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
        let o2 = pdf.len();
        pdf.push_str("2 0 obj\n<< /Type /Pages /Kids [] /Count 0 >>\nendobj\n");
        let o3 = pdf.len();
        pdf.push_str(&format!("3 0 obj\n<< /Length {payload_len} >>\nstream\n"));
        pdf.extend(std::iter::repeat_n('x', payload_len));
        pdf.push_str("\nendstream\nendobj\n");
        let xref = pdf.len();
        pdf.push_str("xref\n0 4\n0000000000 65535 f \n");
        for o in [o1, o2, o3] {
            pdf.push_str(&format!("{o:010} 00000 n \n"));
        }
        pdf.push_str(&format!(
            "trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        ));
        pdf.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::float_cmp
    )]

    use std::io::Cursor;

    use super::testutil::build_pdf;
    use super::*;
    use crate::validate::{DEFAULT_BYTE_BUDGET, ValidationStatus};

    #[test]
    fn valid_and_truncated() {
        let pdf = build_pdf(5000);
        let len = pdf.len() as u64;
        let r = PdfValidator
            .validate(&mut Cursor::new(pdf.clone()), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Valid, "{r:?}");
        let cut = pdf[..pdf.len() - 200].to_vec();
        let len = cut.len() as u64;
        let r = PdfValidator
            .validate(&mut Cursor::new(cut), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
        let r = PdfValidator
            .validate(&mut Cursor::new(b"hello".to_vec()), 5, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Invalid);
    }
}
