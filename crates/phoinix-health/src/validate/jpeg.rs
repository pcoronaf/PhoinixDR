//! JPEG validator: SOI, marker segments, scan data, EOI.

use std::io::SeekFrom;

use super::{FileValidator, ReadSeek, ValidationCheck, ValidationResult, read_at};

/// JPEG structural validator.
#[derive(Debug, Default, Clone, Copy)]
pub struct JpegValidator;

impl FileValidator for JpegValidator {
    fn id(&self) -> &'static str {
        "jpeg"
    }

    fn validate(
        &self,
        stream: &mut dyn ReadSeek,
        len: u64,
        budget: u64,
    ) -> std::io::Result<ValidationResult> {
        let mut checks = Vec::new();
        let head = read_at(stream, 0, usize::try_from(len.min(2)).unwrap_or(0))?;
        let soi = head == [0xFF, 0xD8];
        checks.push(if soi {
            ValidationCheck::pass("SOI marker", "FF D8 present")
        } else {
            ValidationCheck::fail("SOI marker", "missing")
        });
        if !soi {
            return Ok(ValidationResult::from_checks(checks));
        }

        // Walk marker segments until SOS.
        let mut pos = 2u64;
        let mut sof: Option<(u16, u16)> = None;
        let mut reached_sos = false;
        let mut segments = 0u32;
        let limit = len.min(budget);
        while pos + 4 <= limit && segments < 1024 {
            let hdr = read_at(stream, pos, 4)?;
            if hdr.first() != Some(&0xFF) {
                break;
            }
            let marker = hdr.get(1).copied().unwrap_or(0);
            if marker == 0xFF {
                pos += 1;
                continue;
            }
            if marker == 0xD9 {
                break;
            }
            if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
                pos += 2;
                continue;
            }
            let seg_len = u64::from(u16::from_be_bytes([
                hdr.get(2).copied().unwrap_or(0),
                hdr.get(3).copied().unwrap_or(0),
            ]));
            if seg_len < 2 {
                break;
            }
            segments += 1;
            if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF)
                && pos + 9 <= limit
            {
                let sof_bytes = read_at(stream, pos + 5, 4)?;
                let height = u16::from_be_bytes([
                    sof_bytes.first().copied().unwrap_or(0),
                    sof_bytes.get(1).copied().unwrap_or(0),
                ]);
                let width = u16::from_be_bytes([
                    sof_bytes.get(2).copied().unwrap_or(0),
                    sof_bytes.get(3).copied().unwrap_or(0),
                ]);
                sof = Some((width, height));
            }
            if marker == 0xDA {
                reached_sos = true;
                pos += 2 + seg_len;
                break;
            }
            pos += 2 + seg_len;
        }
        checks.push(match sof {
            Some((w, h)) if w > 0 && h > 0 => {
                ValidationCheck::pass("Frame header", format!("{w}×{h} pixels"))
            }
            Some(_) => ValidationCheck::fail("Frame header", "zero dimensions"),
            None => ValidationCheck::fail("Frame header", "no SOF segment before scan data"),
        });
        checks.push(if reached_sos {
            ValidationCheck::pass("Segment sequence", format!("{segments} segments up to SOS"))
        } else {
            ValidationCheck::fail(
                "Segment sequence",
                "no SOS marker found; header segments are broken or truncated",
            )
        });

        // The file must end with EOI (allowing a little trailing garbage).
        let tail_len = len.min(64);
        let tail = read_at(
            stream,
            len - tail_len,
            usize::try_from(tail_len).unwrap_or(0),
        )?;
        let eoi = tail.windows(2).rev().take(32).any(|w| w == [0xFF, 0xD9]);
        checks.push(if eoi {
            ValidationCheck::pass("EOI marker", "FF D9 present at the end")
        } else {
            ValidationCheck::fail(
                "EOI marker",
                "missing; the file appears truncated or its tail was overwritten",
            )
        });

        // Entropy-coded data should not contain long zero runs.
        if reached_sos && pos < len {
            let sample_len = usize::try_from((len - pos).min(4096)).unwrap_or(0);
            stream.seek(SeekFrom::Start(pos))?;
            let mut sample = vec![0u8; sample_len];
            stream.read_exact(&mut sample)?;
            let zeros = sample.iter().filter(|b| **b == 0).count();
            checks.push(if sample_len == 0 || zeros * 2 < sample_len {
                ValidationCheck::pass("Entropy data", "scan data looks like compressed content")
            } else {
                ValidationCheck::fail("Entropy data", "scan data is mostly zero")
            });
        }
        Ok(ValidationResult::from_checks(checks))
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builds a structurally valid (not decodable) baseline JPEG.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    pub fn build_jpeg(width: u16, height: u16, scan_len: usize) -> Vec<u8> {
        let mut j = vec![0xFF, 0xD8];
        // APP0 JFIF
        j.extend_from_slice(&[
            0xFF, 0xE0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 1, 0, 0, 1, 0, 1, 0, 0,
        ]);
        // DQT
        j.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
        j.extend(std::iter::repeat_n(16u8, 64));
        // SOF0: 8-bit, 1 component
        j.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
        j.extend_from_slice(&height.to_be_bytes());
        j.extend_from_slice(&width.to_be_bytes());
        j.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
        // DHT (tiny)
        j.extend_from_slice(&[
            0xFF, 0xC4, 0x00, 0x14, 0x00, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
        // SOS
        j.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
        for i in 0..scan_len {
            j.push(0x10 | ((i * 37) as u8 & 0x6F));
        }
        j.extend_from_slice(&[0xFF, 0xD9]);
        j
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

    use super::testutil::build_jpeg;
    use super::*;
    use crate::validate::{DEFAULT_BYTE_BUDGET, ValidationStatus};

    #[test]
    fn valid_truncated_and_invalid() {
        let jpg = build_jpeg(640, 480, 10_000);
        let len = jpg.len() as u64;
        let r = JpegValidator
            .validate(&mut Cursor::new(jpg.clone()), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Valid, "{r:?}");
        assert!(r.checks.iter().any(|c| c.detail == "640×480 pixels"));

        let cut = jpg[..jpg.len() - 3000].to_vec();
        let len = cut.len() as u64;
        let r = JpegValidator
            .validate(&mut Cursor::new(cut), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(r.checks.iter().any(|c| c.name == "EOI marker" && !c.passed));

        // Zeroed tail cluster: scan data zero and EOI gone.
        let mut wiped = jpg.clone();
        for b in wiped[200..].iter_mut() {
            *b = 0;
        }
        let len = wiped.len() as u64;
        let r = JpegValidator
            .validate(&mut Cursor::new(wiped), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);

        let r = JpegValidator
            .validate(&mut Cursor::new(vec![0u8; 100]), 100, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Invalid);
    }
}
