//! PNG validator: signature, chunk walk with CRC verification, IEND.

use phoinix_core::bytes::ByteView;

use super::{FileValidator, ReadSeek, ValidationCheck, ValidationResult, read_at};

/// PNG structural validator.
#[derive(Debug, Default, Clone, Copy)]
pub struct PngValidator;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

impl FileValidator for PngValidator {
    fn id(&self) -> &'static str {
        "png"
    }

    fn validate(
        &self,
        stream: &mut dyn ReadSeek,
        len: u64,
        budget: u64,
    ) -> std::io::Result<ValidationResult> {
        let mut checks = Vec::new();
        let head = read_at(stream, 0, usize::try_from(len.min(8)).unwrap_or(0))?;
        let sig_ok = head == SIGNATURE;
        checks.push(if sig_ok {
            ValidationCheck::pass("PNG signature", "present")
        } else {
            ValidationCheck::fail("PNG signature", "missing")
        });
        if !sig_ok {
            return Ok(ValidationResult::from_checks(checks));
        }

        let mut pos = 8u64;
        let mut chunks = 0u32;
        let mut ihdr: Option<(u32, u32)> = None;
        let mut iend = false;
        let mut crc_bad = 0u32;
        let mut truncated = false;
        let limit = len.min(budget);
        while pos + 12 <= limit && chunks < 100_000 {
            let hdr = read_at(stream, pos, 8)?;
            let view = ByteView::new(&hdr);
            let length = u64::from(u32::from_be_bytes(view.array::<4>(0).unwrap_or([0; 4])));
            let kind = view.array::<4>(4).unwrap_or([0; 4]);
            let Some(chunk_end) = pos.checked_add(12).and_then(|p| p.checked_add(length)) else {
                truncated = true;
                break;
            };
            if chunk_end > limit {
                truncated = true;
                break;
            }
            // CRC covers type + data; verify chunks up to 16 MiB.
            if length <= 16 * 1024 * 1024 {
                let body = read_at(stream, pos + 4, usize::try_from(4 + length).unwrap_or(0))?;
                let stored = u32::from_be_bytes(
                    read_at(stream, pos + 8 + length, 4)?
                        .try_into()
                        .unwrap_or([0; 4]),
                );
                if crc32fast::hash(&body) != stored {
                    crc_bad += 1;
                }
            }
            if &kind == b"IHDR" && length >= 8 {
                let d = read_at(stream, pos + 8, 8)?;
                let v = ByteView::new(&d);
                ihdr = Some((
                    u32::from_be_bytes(v.array::<4>(0).unwrap_or([0; 4])),
                    u32::from_be_bytes(v.array::<4>(4).unwrap_or([0; 4])),
                ));
            }
            chunks += 1;
            pos = chunk_end;
            if &kind == b"IEND" {
                iend = true;
                break;
            }
        }
        checks.push(match ihdr {
            Some((w, h)) if w > 0 && h > 0 => {
                ValidationCheck::pass("IHDR", format!("{w}×{h} pixels"))
            }
            _ => ValidationCheck::fail("IHDR", "missing or zero-sized image header"),
        });
        checks.push(if truncated {
            ValidationCheck::fail(
                "Chunk sequence",
                format!("chunk at offset {pos} extends beyond the file"),
            )
        } else {
            ValidationCheck::pass("Chunk sequence", format!("{chunks} chunks"))
        });
        checks.push(if crc_bad == 0 {
            ValidationCheck::pass("Chunk CRC32", "all verified chunks match")
        } else {
            ValidationCheck::fail(
                "Chunk CRC32",
                format!("{crc_bad} chunks have CRC mismatches"),
            )
        });
        checks.push(if iend {
            ValidationCheck::pass("IEND", "present")
        } else {
            ValidationCheck::fail("IEND", "missing; the file is truncated")
        });
        Ok(ValidationResult::from_checks(checks))
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builds a valid (uncompressed-stored-deflate) PNG.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        c.extend_from_slice(&body);
        c.extend_from_slice(&crc32fast::hash(&body).to_be_bytes());
        c
    }

    /// Stored (uncompressed) zlib stream.
    fn zlib_stored(raw: &[u8]) -> Vec<u8> {
        let mut z = vec![0x78, 0x01];
        let mut chunks = raw.chunks(65_535).peekable();
        if raw.is_empty() {
            z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
        }
        while let Some(part) = chunks.next() {
            z.push(u8::from(chunks.peek().is_none()));
            z.extend_from_slice(&(part.len() as u16).to_le_bytes());
            z.extend_from_slice(&(!(part.len() as u16)).to_le_bytes());
            z.extend_from_slice(part);
        }
        // Adler-32
        let (mut a, mut b) = (1u32, 0u32);
        for byte in raw {
            a = (a + u32::from(*byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        z
    }

    pub fn build_png(width: u32, height: u32) -> Vec<u8> {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 0, 0, 0, 0]); // 8-bit grayscale
        png.extend(chunk(b"IHDR", &ihdr));
        let mut raw = Vec::new();
        for y in 0..height {
            raw.push(0);
            for x in 0..width {
                raw.push(((x * 7 + y * 13) % 251) as u8);
            }
        }
        png.extend(chunk(b"IDAT", &zlib_stored(&raw)));
        png.extend(chunk(b"IEND", &[]));
        png
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

    use super::testutil::build_png;
    use super::*;
    use crate::validate::{DEFAULT_BYTE_BUDGET, ValidationStatus};

    #[test]
    fn valid_corrupt_and_truncated() {
        let png = build_png(64, 48);
        let len = png.len() as u64;
        let r = PngValidator
            .validate(&mut Cursor::new(png.clone()), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Valid, "{r:?}");

        let mut bad = png.clone();
        bad[100] ^= 0xFF;
        let r = PngValidator
            .validate(&mut Cursor::new(bad), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(
            r.checks
                .iter()
                .any(|c| c.name == "Chunk CRC32" && !c.passed)
        );

        let cut = png[..png.len() - 20].to_vec();
        let len = cut.len() as u64;
        let r = PngValidator
            .validate(&mut Cursor::new(cut), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(r.checks.iter().any(|c| c.name == "IEND" && !c.passed));
    }
}
