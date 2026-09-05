//! Assemblers: given a matched header, walk the file structure to find
//! where the file ends and how sound it looks.
//!
//! Every assembler is defensive: bounded loops, checked offsets, and a
//! [`CarveError::Truncated`] from the probe is turned into a damaged,
//! open-ended assembly rather than an error.

pub(crate) mod bmp;
pub(crate) mod generic;
pub(crate) mod gif;
pub(crate) mod jpeg;
pub(crate) mod mp4;
pub(crate) mod pdf;
pub(crate) mod png;
pub(crate) mod riff;
pub(crate) mod sevenzip;
pub(crate) mod sqlite;
pub(crate) mod zip;

use phoinix_health::{ValidationCheck, ValidationStatus};

use crate::CarveError;
use crate::probe::Probe;
use crate::signature::{AssemblerKind, CarveSignature};

/// The outcome of assembling one hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    /// Bytes from the header to the end of the file (or to the bound when
    /// the end is unknown).
    pub length: u64,
    /// Whether the structure determined the end.
    pub end_known: bool,
    /// Structural checks performed.
    pub checks: Vec<ValidationCheck>,
    /// Overall status derived from the checks.
    pub status: ValidationStatus,
    /// A refined type id (`docx` for a ZIP that is a Word document).
    pub type_id: Option<String>,
    /// A refined type name.
    pub type_name: Option<String>,
    /// A refined extension (`wav` for a RIFF WAVE).
    pub extension: Option<String>,
}

impl Assembly {
    /// An assembly whose status follows from its checks.
    #[must_use]
    pub fn from_checks(length: u64, end_known: bool, checks: Vec<ValidationCheck>) -> Self {
        let status = if checks.iter().all(|c| c.passed) {
            ValidationStatus::Valid
        } else if checks.first().is_some_and(|c| !c.passed) {
            ValidationStatus::Invalid
        } else {
            ValidationStatus::Damaged
        };
        Self {
            length,
            end_known,
            checks,
            status,
            type_id: None,
            type_name: None,
            extension: None,
        }
    }

    /// Overrides the status.
    #[must_use]
    pub const fn with_status(mut self, status: ValidationStatus) -> Self {
        self.status = status;
        self
    }
}

/// Determines the extent of a file from its header.
pub trait Assembler: Send + Sync {
    /// Assembles the file starting at `start`, never reading past
    /// `start + max_len`. Returns `None` when the header is a false
    /// positive (the bytes cannot be this type at all).
    ///
    /// # Errors
    ///
    /// Returns block errors; truncation at the region end is not an error.
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError>;
}

/// The assembler for a signature.
#[must_use]
pub fn assembler_for(signature: &CarveSignature) -> Box<dyn Assembler> {
    match signature.assembler {
        AssemblerKind::Jpeg => Box::new(jpeg::JpegAssembler),
        AssemblerKind::Png => Box::new(png::PngAssembler),
        AssemblerKind::Gif => Box::new(gif::GifAssembler),
        AssemblerKind::Bmp => Box::new(bmp::BmpAssembler),
        AssemblerKind::Pdf => Box::new(pdf::PdfAssembler),
        AssemblerKind::Zip => Box::new(zip::ZipAssembler),
        AssemblerKind::Sqlite => Box::new(sqlite::SqliteAssembler),
        AssemblerKind::Riff => Box::new(riff::RiffAssembler),
        AssemblerKind::Mp4 => Box::new(mp4::Mp4Assembler),
        AssemblerKind::SevenZip => Box::new(sevenzip::SevenZipAssembler),
        AssemblerKind::Footer => Box::new(generic::FooterAssembler {
            footer: signature.footer.clone().unwrap_or_default(),
            header_span: signature.header_span() as u64,
        }),
        AssemblerKind::HeaderOnly => Box::new(generic::HeaderOnlyAssembler),
    }
}

/// Runs `assemble` and converts a truncation at the region end into an
/// open-ended, damaged assembly covering what was readable.
pub(crate) fn tolerate_truncation(
    result: Result<Option<Assembly>, CarveError>,
    start: u64,
    probe_limit: u64,
    checks_so_far: Vec<ValidationCheck>,
) -> Result<Option<Assembly>, CarveError> {
    match result {
        // Truncated before the signature itself was confirmed: not a file.
        Err(CarveError::Truncated { .. }) if checks_so_far.is_empty() => Ok(None),
        Err(CarveError::Truncated { .. }) => {
            let mut checks = checks_so_far;
            checks.push(ValidationCheck::fail(
                "region end",
                "the structure runs past the end of the scanned region",
            ));
            let length = probe_limit.saturating_sub(start);
            Ok(Some(
                Assembly::from_checks(length, false, checks).with_status(ValidationStatus::Damaged),
            ))
        }
        other => other,
    }
}

/// Little-endian `u32` at `i`, if in bounds.
pub(crate) fn le32(b: &[u8], i: usize) -> Option<u32> {
    let s = b.get(i..i.checked_add(4)?)?;
    Some(u32::from_le_bytes([
        *s.first()?,
        *s.get(1)?,
        *s.get(2)?,
        *s.get(3)?,
    ]))
}

/// Little-endian `u64` at `i`, if in bounds.
pub(crate) fn le64(b: &[u8], i: usize) -> Option<u64> {
    let s = b.get(i..i.checked_add(8)?)?;
    let mut a = [0u8; 8];
    for (slot, byte) in a.iter_mut().zip(s) {
        *slot = *byte;
    }
    Some(u64::from_le_bytes(a))
}

/// Big-endian `u16` at `i`, if in bounds.
pub(crate) fn be16(b: &[u8], i: usize) -> Option<u16> {
    let s = b.get(i..i.checked_add(2)?)?;
    Some(u16::from_be_bytes([*s.first()?, *s.get(1)?]))
}

/// Big-endian `u32` at `i`, if in bounds.
pub(crate) fn be32(b: &[u8], i: usize) -> Option<u32> {
    let s = b.get(i..i.checked_add(4)?)?;
    Some(u32::from_be_bytes([
        *s.first()?,
        *s.get(1)?,
        *s.get(2)?,
        *s.get(3)?,
    ]))
}

/// Bounds a length by the readable region and the maximum.
pub(crate) fn clamp_len(start: u64, wanted: u64, max_len: u64, limit: u64) -> u64 {
    wanted.min(max_len).min(limit.saturating_sub(start))
}

#[cfg(test)]
pub(crate) mod testutil {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]
    use phoinix_block::MemoryReader;

    use super::*;

    /// Assembles `data` placed at offset `start` inside `padding` zeros on
    /// each side.
    pub fn run(assembler: &dyn Assembler, data: &[u8], tail: &[u8]) -> Option<Assembly> {
        let start = 4096u64;
        let mut image = vec![0u8; start as usize];
        image.extend_from_slice(data);
        image.extend_from_slice(tail);
        let reader = MemoryReader::new(image.clone());
        let mut probe = Probe::new(&reader, image.len() as u64);
        assembler
            .assemble(&mut probe, start, 64 * 1024 * 1024)
            .unwrap()
    }
}
