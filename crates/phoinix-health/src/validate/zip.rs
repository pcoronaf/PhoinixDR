//! ZIP validator (also OOXML/ODF containers).

use std::io::SeekFrom;

use phoinix_core::bytes::ByteView;

use super::{
    FileTypeDetection, FileValidator, ReadSeek, ValidationCheck, ValidationResult, read_at,
};

const EOCD_SIG: u32 = 0x0605_4B50;
const CENTRAL_SIG: u32 = 0x0201_4B50;
const LOCAL_SIG: u32 = 0x0403_4B50;
const MAX_ENTRIES_CHECKED: usize = 512;
const MAX_CRC_BYTES: u64 = 64 * 1024 * 1024;

/// ZIP structural validator.
#[derive(Debug, Default, Clone, Copy)]
pub struct ZipValidator;

struct CentralEntry {
    name: String,
    method: u16,
    crc: u32,
    compressed_size: u64,
    local_offset: u64,
}

fn find_eocd(stream: &mut dyn ReadSeek, len: u64) -> std::io::Result<Option<(u64, Vec<u8>)>> {
    // EOCD is 22 bytes plus a comment of at most 65535 bytes.
    let tail_len = len.min(22 + 65_535);
    if tail_len < 22 {
        return Ok(None);
    }
    let start = len - tail_len;
    let tail = read_at(stream, start, usize::try_from(tail_len).unwrap_or(0))?;
    let view = ByteView::new(&tail);
    let mut pos = tail.len() - 22;
    loop {
        if view.u32_le(pos) == Some(EOCD_SIG) {
            let comment_len = usize::from(view.u16_le(pos + 20).unwrap_or(0));
            if pos + 22 + comment_len == tail.len() {
                return Ok(Some((
                    start + pos as u64,
                    tail.get(pos..).unwrap_or(&[]).to_vec(),
                )));
            }
        }
        if pos == 0 {
            return Ok(None);
        }
        pos -= 1;
    }
}

fn read_central(
    stream: &mut dyn ReadSeek,
    offset: u64,
    size: u64,
    len: u64,
) -> std::io::Result<Option<Vec<CentralEntry>>> {
    if offset.checked_add(size).is_none_or(|end| end > len) || size > 64 * 1024 * 1024 {
        return Ok(None);
    }
    let bytes = read_at(stream, offset, usize::try_from(size).unwrap_or(0))?;
    let view = ByteView::new(&bytes);
    let mut entries = Vec::new();
    let mut pos = 0usize;
    while pos + 46 <= bytes.len() {
        if view.u32_le(pos) != Some(CENTRAL_SIG) {
            return Ok(None);
        }
        let method = view.u16_le(pos + 10).unwrap_or(0);
        let crc = view.u32_le(pos + 16).unwrap_or(0);
        let compressed_size = u64::from(view.u32_le(pos + 20).unwrap_or(0));
        let name_len = usize::from(view.u16_le(pos + 28).unwrap_or(0));
        let extra_len = usize::from(view.u16_le(pos + 30).unwrap_or(0));
        let comment_len = usize::from(view.u16_le(pos + 32).unwrap_or(0));
        let local_offset = u64::from(view.u32_le(pos + 42).unwrap_or(0));
        let name = view
            .slice(pos + 46, name_len)
            .map(|n| String::from_utf8_lossy(n).into_owned())
            .unwrap_or_default();
        entries.push(CentralEntry {
            name,
            method,
            crc,
            compressed_size,
            local_offset,
        });
        pos += 46 + name_len + extra_len + comment_len;
    }
    Ok(Some(entries))
}

/// Refines a ZIP into DOCX/XLSX/PPTX/ODF when the central directory names
/// reveal one.
pub fn refine_container(stream: &mut dyn ReadSeek, len: u64) -> Option<FileTypeDetection> {
    let (_, eocd) = find_eocd(stream, len).ok()??;
    let view = ByteView::new(&eocd);
    let size = u64::from(view.u32_le(12)?);
    let offset = u64::from(view.u32_le(16)?);
    let entries = read_central(stream, offset, size, len).ok()??;
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    let has = |prefix: &str| names.iter().any(|n| n.starts_with(prefix));
    let det = |id: &str, name: &str, ext: &str| FileTypeDetection {
        id: id.into(),
        name: name.into(),
        extension: ext.into(),
    };
    if names.contains(&"[Content_Types].xml") {
        if has("word/") {
            return Some(det("docx", "Word document (DOCX)", "docx"));
        }
        if has("xl/") {
            return Some(det("xlsx", "Excel workbook (XLSX)", "xlsx"));
        }
        if has("ppt/") {
            return Some(det("pptx", "PowerPoint presentation (PPTX)", "pptx"));
        }
    }
    if names.contains(&"mimetype") && names.contains(&"content.xml") {
        return Some(det("odf", "OpenDocument", "odt"));
    }
    if names.contains(&"META-INF/MANIFEST.MF") {
        return Some(det("jar", "Java archive (JAR)", "jar"));
    }
    None
}

impl FileValidator for ZipValidator {
    fn id(&self) -> &'static str {
        "zip"
    }

    fn validate(
        &self,
        stream: &mut dyn ReadSeek,
        len: u64,
        budget: u64,
    ) -> std::io::Result<ValidationResult> {
        let mut checks = Vec::new();
        let head = read_at(stream, 0, usize::try_from(len.min(4)).unwrap_or(0))?;
        let sig_ok = head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06");
        checks.push(if sig_ok {
            ValidationCheck::pass("ZIP signature", "local file header signature present")
        } else {
            ValidationCheck::fail("ZIP signature", "missing PK signature")
        });
        if !sig_ok {
            return Ok(ValidationResult::from_checks(checks));
        }

        let Some((eocd_offset, eocd)) = find_eocd(stream, len)? else {
            checks.push(ValidationCheck::fail(
                "End of central directory",
                "not found near the end of the file (truncated?)",
            ));
            return Ok(ValidationResult::from_checks(checks));
        };
        checks.push(ValidationCheck::pass(
            "End of central directory",
            format!("found at offset {eocd_offset}"),
        ));
        let view = ByteView::new(&eocd);
        let entry_count = u64::from(view.u16_le(10).unwrap_or(0));
        let dir_size = u64::from(view.u32_le(12).unwrap_or(0));
        let dir_offset = u64::from(view.u32_le(16).unwrap_or(0));

        let Some(entries) = read_central(stream, dir_offset, dir_size, len)? else {
            checks.push(ValidationCheck::fail(
                "Central directory",
                "unreadable or inconsistent",
            ));
            return Ok(ValidationResult::from_checks(checks));
        };
        let count_ok = entries.len() as u64 == entry_count || entry_count == 0xFFFF;
        checks.push(if count_ok {
            ValidationCheck::pass("Central directory", format!("{} entries", entries.len()))
        } else {
            ValidationCheck::fail(
                "Central directory",
                format!("{} entries found, {entry_count} declared", entries.len()),
            )
        });

        // Local headers and CRCs (stored entries only; deflated entries would
        // need inflation, which is deferred to the carving milestone).
        let mut local_bad = 0usize;
        let mut crc_checked = 0usize;
        let mut crc_bad = 0usize;
        let mut crc_bytes = 0u64;
        for entry in entries.iter().take(MAX_ENTRIES_CHECKED) {
            if entry.local_offset.checked_add(30).is_none_or(|e| e > len) {
                local_bad += 1;
                continue;
            }
            let local = read_at(stream, entry.local_offset, 30)?;
            let lv = ByteView::new(&local);
            if lv.u32_le(0) != Some(LOCAL_SIG) {
                local_bad += 1;
                continue;
            }
            let name_len = u64::from(lv.u16_le(26).unwrap_or(0));
            let extra_len = u64::from(lv.u16_le(28).unwrap_or(0));
            let data_offset = entry.local_offset + 30 + name_len + extra_len;
            if entry.method == 0
                && entry.compressed_size > 0
                && crc_bytes + entry.compressed_size <= MAX_CRC_BYTES.min(budget)
            {
                if data_offset
                    .checked_add(entry.compressed_size)
                    .is_none_or(|e| e > len)
                {
                    local_bad += 1;
                    continue;
                }
                stream.seek(SeekFrom::Start(data_offset))?;
                let mut hasher = crc32fast::Hasher::new();
                let mut remaining = entry.compressed_size;
                let mut buf = vec![0u8; 64 * 1024];
                while remaining > 0 {
                    let want = usize::try_from(remaining.min(buf.len() as u64)).unwrap_or(0);
                    let slot = buf.get_mut(..want).unwrap_or(&mut []);
                    stream.read_exact(slot)?;
                    hasher.update(slot);
                    remaining -= want as u64;
                }
                crc_bytes += entry.compressed_size;
                crc_checked += 1;
                if hasher.finalize() != entry.crc {
                    crc_bad += 1;
                }
            }
        }
        checks.push(if local_bad == 0 {
            ValidationCheck::pass(
                "Local headers",
                "every checked entry has a valid local header",
            )
        } else {
            ValidationCheck::fail(
                "Local headers",
                format!("{local_bad} entries have missing or invalid local headers"),
            )
        });
        if crc_checked > 0 {
            checks.push(if crc_bad == 0 {
                ValidationCheck::pass(
                    "Entry CRC32",
                    format!("{crc_checked} stored entries verified"),
                )
            } else {
                ValidationCheck::fail(
                    "Entry CRC32",
                    format!("{crc_bad} of {crc_checked} stored entries have CRC mismatches"),
                )
            });
        }
        Ok(ValidationResult::from_checks(checks))
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builds small ZIP files (stored entries) without external crates.

    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        missing_docs
    )]

    pub fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = out.len() as u32;
            let crc = crc32fast::hash(data);
            let mut local = Vec::new();
            local.extend_from_slice(b"PK\x03\x04");
            local.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            local.extend_from_slice(&crc.to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&local);
            out.extend_from_slice(data);

            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let dir_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
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

    use super::testutil::build_zip;
    use super::*;
    use crate::validate::{DEFAULT_BYTE_BUDGET, ValidationStatus, examine};

    #[test]
    fn valid_zip_and_docx_detection() {
        let zip = build_zip(&[
            ("[Content_Types].xml", b"<Types/>"),
            ("word/document.xml", b"<w:document/>"),
        ]);
        let len = zip.len() as u64;
        let mut c = Cursor::new(zip);
        let r = ZipValidator
            .validate(&mut c, len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Valid, "{r:?}");
        let e = examine(&mut c, len, DEFAULT_BYTE_BUDGET).unwrap();
        assert_eq!(e.detected_type.unwrap().id, "docx");
    }

    #[test]
    fn truncated_zip_is_damaged_and_corrupt_data_fails_crc() {
        let zip = build_zip(&[("a.txt", b"hello world")]);
        let cut = zip[..zip.len() - 10].to_vec();
        let len = cut.len() as u64;
        let r = ZipValidator
            .validate(&mut Cursor::new(cut), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);

        let mut bad = zip.clone();
        bad[37] ^= 0xFF; // inside "hello world" (30-byte header + 5-byte name)
        let len = bad.len() as u64;
        let r = ZipValidator
            .validate(&mut Cursor::new(bad), len, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(
            r.checks
                .iter()
                .any(|c| c.name == "Entry CRC32" && !c.passed)
        );

        let r = ZipValidator
            .validate(&mut Cursor::new(b"nope".to_vec()), 4, DEFAULT_BYTE_BUDGET)
            .unwrap();
        assert_eq!(r.status, ValidationStatus::Invalid);
    }
}
