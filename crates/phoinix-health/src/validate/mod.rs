//! Minimal structural validators (M4 baseline).
//!
//! Header detection alone is insufficient: a perfect JPEG header says
//! nothing about the last cluster. These validators walk enough of the
//! structure to tell *valid*, *mostly valid*, *damaged* and *invalid* apart,
//! and feed [`ContentEvidence`]. The full plugin
//! validator framework belongs to the carving milestone.

mod jpeg;
mod magic;
mod pdf;
mod png;
mod zip;

use std::io::{Read, Seek, SeekFrom};

use serde::{Deserialize, Serialize};

pub use magic::{SIGNATURES, Signature, detect_type};

use crate::evidence::ContentEvidence;

/// A `Read + Seek` stream of candidate content.
pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

/// Largest stream a validator will walk end to end.
pub const DEFAULT_BYTE_BUDGET: u64 = 256 * 1024 * 1024;

/// Detected file type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTypeDetection {
    /// Short identifier (`jpeg`, `zip`, `docx`, …).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Typical extension.
    pub extension: String,
}

/// Outcome of a structural validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationStatus {
    /// Every check passed.
    Valid,
    /// Core structure intact; minor inconsistencies.
    MostlyValid,
    /// Structure recognised but broken (truncated, bad CRCs).
    Damaged,
    /// Not the claimed structure at all.
    Invalid,
    /// No validator could assess the content.
    Unknown,
}

/// One check performed by a validator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCheck {
    /// Name of the check.
    pub name: String,
    /// Whether it passed.
    pub passed: bool,
    /// Detail text.
    pub detail: String,
}

impl ValidationCheck {
    /// A passed check.
    #[must_use]
    pub fn pass(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            passed: true,
            detail: detail.into(),
        }
    }

    /// A failed check.
    #[must_use]
    pub fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            passed: false,
            detail: detail.into(),
        }
    }
}

/// Result of validating content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationResult {
    /// Overall status.
    pub status: ValidationStatus,
    /// Individual checks.
    pub checks: Vec<ValidationCheck>,
}

impl ValidationResult {
    /// Builds a result whose status follows from the checks: all passed →
    /// valid; the first (signature) check failed → invalid; otherwise
    /// damaged.
    #[must_use]
    pub fn from_checks(checks: Vec<ValidationCheck>) -> Self {
        let status = if checks.iter().all(|c| c.passed) {
            ValidationStatus::Valid
        } else if checks.first().is_some_and(|c| !c.passed) {
            ValidationStatus::Invalid
        } else {
            ValidationStatus::Damaged
        };
        Self { status, checks }
    }

    /// A result with a status forced by the caller.
    #[must_use]
    pub fn with_status(status: ValidationStatus, checks: Vec<ValidationCheck>) -> Self {
        Self { status, checks }
    }
}

/// Validates the structure of one file family.
pub trait FileValidator: Send + Sync {
    /// Type identifier this validator handles (matches [`Signature::id`]).
    fn id(&self) -> &'static str;

    /// Validates `stream`, whose length is `len`.
    fn validate(
        &self,
        stream: &mut dyn ReadSeek,
        len: u64,
        budget: u64,
    ) -> std::io::Result<ValidationResult>;
}

fn validator_for(id: &str) -> Option<Box<dyn FileValidator>> {
    Some(match id {
        "jpeg" => Box::new(jpeg::JpegValidator),
        "png" => Box::new(png::PngValidator),
        "pdf" => Box::new(pdf::PdfValidator),
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "jar" => {
            Box::new(zip::ZipValidator)
        }
        _ => return None,
    })
}

/// Number of 4 KiB blocks sampled for the zero-fill ratio.
const ZERO_SAMPLE_BLOCKS: u64 = 64;

/// Examines `stream` and produces content evidence: type detection, a
/// structural validation when a validator exists, and a zero-fill sample.
///
/// # Errors
///
/// Returns I/O errors from the stream.
pub fn examine(
    stream: &mut dyn ReadSeek,
    len: u64,
    budget: u64,
) -> std::io::Result<ContentEvidence> {
    let mut evidence = ContentEvidence::default();
    if len == 0 {
        return Ok(evidence);
    }
    let head_len = usize::try_from(len.min(4096)).unwrap_or(4096);
    let mut head = vec![0u8; head_len];
    stream.seek(SeekFrom::Start(0))?;
    stream.read_exact(&mut head)?;
    evidence.bytes_examined = head.len() as u64;

    // Refine ZIP into OOXML/ODF families using the first local header name.
    let detected = detect_type(&head).map(|sig| {
        if sig.id == "zip" {
            zip::refine_container(stream, len).unwrap_or_else(|| sig.detection())
        } else {
            sig.detection()
        }
    });

    if let Some(det) = &detected
        && let Some(validator) = validator_for(&det.id)
    {
        stream.seek(SeekFrom::Start(0))?;
        let result = validator.validate(stream, len, budget)?;
        evidence.bytes_examined = evidence.bytes_examined.max(len.min(budget));
        evidence.validation = Some(result);
    }
    evidence.detected_type = detected;

    // Zero-fill sampling: up to 64 blocks of 4 KiB spread over the stream.
    let block = 4096u64;
    let blocks = len.div_ceil(block);
    let samples = blocks.min(ZERO_SAMPLE_BLOCKS);
    let mut zero = 0u64;
    let mut buf = vec![0u8; 4096];
    for i in 0..samples {
        let index = if blocks <= ZERO_SAMPLE_BLOCKS {
            i
        } else {
            i * blocks / samples
        };
        let offset = index * block;
        let want = usize::try_from((len - offset).min(block)).unwrap_or(4096);
        stream.seek(SeekFrom::Start(offset))?;
        let slot = buf.get_mut(..want).unwrap_or(&mut []);
        stream.read_exact(slot)?;
        if slot.iter().all(|b| *b == 0) {
            zero += 1;
        }
    }
    if samples > 0 {
        evidence.zero_block_ratio = Some(zero as f64 / samples as f64);
        evidence.bytes_examined = evidence.bytes_examined.max(samples * block).min(len);
    }
    Ok(evidence)
}

/// Reads exactly `len` bytes at `offset`.
fn read_at(stream: &mut dyn ReadSeek, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    stream.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(buf)
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

    use super::*;

    #[test]
    fn examine_unknown_content_samples_zeros() {
        let mut data = vec![0u8; 20_000];
        data[10_000] = 1;
        let mut c = Cursor::new(data);
        let e = examine(&mut c, 20_000, DEFAULT_BYTE_BUDGET).unwrap();
        assert!(e.detected_type.is_none());
        assert!(e.validation.is_none());
        assert_eq!(e.zero_block_ratio, Some(0.8));
    }

    #[test]
    fn examine_empty() {
        let mut c = Cursor::new(Vec::new());
        let e = examine(&mut c, 0, DEFAULT_BYTE_BUDGET).unwrap();
        assert_eq!(e.bytes_examined, 0);
        assert!(e.zero_block_ratio.is_none());
    }
}
