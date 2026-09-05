//! ZIP family: local headers, central directory and the end record.
//!
//! Data-descriptor entries (streamed writers, OOXML from some producers)
//! carry no sizes in the local header; the descriptor is located by
//! searching for the next signature and checking the sizes it declares.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len, tolerate_truncation};
use crate::CarveError;
use crate::probe::Probe;

const LOCAL: u32 = 0x0403_4B50;
const CENTRAL: u32 = 0x0201_4B50;
const EOCD: u32 = 0x0605_4B50;
const EOCD64: u32 = 0x0606_4B50;
const LOCATOR64: u32 = 0x0706_4B50;
const DESCRIPTOR: u32 = 0x0807_4B50;
const MAX_ENTRIES: usize = 1_000_000;
const MAX_NAMES: usize = 64;

/// ZIP assembler.
pub struct ZipAssembler;

impl Assembler for ZipAssembler {
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

/// Reads the zip64 sizes from the extra field, if present.
fn zip64_sizes(extra: &[u8], csize: u32, usize_: u32) -> Option<(u64, u64)> {
    let mut i = 0usize;
    while i.saturating_add(4) <= extra.len() {
        let id = u16::from_le_bytes([*extra.get(i)?, *extra.get(i + 1)?]);
        let len = usize::from(u16::from_le_bytes([*extra.get(i + 2)?, *extra.get(i + 3)?]));
        let body = extra.get(i + 4..i.saturating_add(4).saturating_add(len))?;
        if id == 0x0001 {
            let mut fields = body.chunks_exact(8).map(|c| {
                let mut a = [0u8; 8];
                a.copy_from_slice(c);
                u64::from_le_bytes(a)
            });
            let mut u = u64::from(usize_);
            let mut c = u64::from(csize);
            if usize_ == u32::MAX {
                u = fields.next()?;
            }
            if csize == u32::MAX {
                c = fields.next()?;
            }
            return Some((c, u));
        }
        i = i.saturating_add(4).saturating_add(len);
    }
    None
}

/// Classifies a container by its entry names.
fn classify(names: &[String]) -> Option<(&'static str, &'static str, &'static str)> {
    let has = |prefix: &str| names.iter().any(|n| n.starts_with(prefix));
    let contains = |exact: &str| names.iter().any(|n| n == exact);
    if contains("[Content_Types].xml") {
        if has("word/") {
            return Some(("docx", "Word document (DOCX)", "docx"));
        }
        if has("xl/") {
            return Some(("xlsx", "Excel workbook (XLSX)", "xlsx"));
        }
        if has("ppt/") {
            return Some(("pptx", "PowerPoint presentation (PPTX)", "pptx"));
        }
    }
    if contains("mimetype") && contains("content.xml") {
        return Some(("odf", "OpenDocument", "odt"));
    }
    if contains("META-INF/MANIFEST.MF") {
        return Some(("jar", "Java archive (JAR)", "jar"));
    }
    None
}

fn walk(
    probe: &mut Probe<'_>,
    start: u64,
    max_len: u64,
    checks: &mut Vec<ValidationCheck>,
) -> Result<Option<Assembly>, CarveError> {
    let end = start.saturating_add(clamp_len(start, max_len, max_len, probe.limit()));
    if probe.u32_le(start)? != LOCAL {
        return Ok(None);
    }
    checks.push(ValidationCheck::pass(
        "signature",
        "local file header present",
    ));
    let mut pos = start;
    let mut locals = 0usize;
    let mut centrals = 0usize;
    let mut names: Vec<String> = Vec::new();
    let mut descriptors_resolved = 0usize;
    for _ in 0..MAX_ENTRIES {
        if pos.saturating_add(4) > end {
            break;
        }
        let sig = probe.u32_le(pos)?;
        match sig {
            LOCAL => {
                locals += 1;
                let flags = probe.u16_le(pos.saturating_add(6))?;
                let csize32 = probe.u32_le(pos.saturating_add(18))?;
                let usize32 = probe.u32_le(pos.saturating_add(22))?;
                let nlen = u64::from(probe.u16_le(pos.saturating_add(26))?);
                let xlen = u64::from(probe.u16_le(pos.saturating_add(28))?);
                let header_len = 30u64.saturating_add(nlen).saturating_add(xlen);
                if names.len() < MAX_NAMES && nlen > 0 && nlen < 4096 {
                    let name =
                        probe.read(pos.saturating_add(30), usize::try_from(nlen).unwrap_or(0))?;
                    names.push(String::from_utf8_lossy(&name).into_owned());
                }
                let mut csize = u64::from(csize32);
                if csize32 == u32::MAX || usize32 == u32::MAX {
                    let extra = probe.read(
                        pos.saturating_add(30).saturating_add(nlen),
                        usize::try_from(xlen).unwrap_or(0),
                    )?;
                    if let Some((c, _)) = zip64_sizes(&extra, csize32, usize32) {
                        csize = c;
                    }
                }
                let data_start = pos.saturating_add(header_len);
                if flags & 0x0008 != 0 && csize == 0 {
                    // Data descriptor: find the next signature and check the
                    // descriptor right before it.
                    let Some((next, descriptor_len)) = find_descriptor(probe, data_start, end)?
                    else {
                        checks.push(ValidationCheck::fail(
                            "entries",
                            format!(
                                "entry {locals} uses a data descriptor that could not be located"
                            ),
                        ));
                        return Ok(Some(damaged(data_start - start, checks, locals + centrals)));
                    };
                    descriptors_resolved += 1;
                    let _ = descriptor_len;
                    pos = next;
                } else {
                    let mut next = data_start.saturating_add(csize);
                    if flags & 0x0008 != 0 && next.saturating_add(4) <= end {
                        // Sizes present and a descriptor still follows.
                        let s = probe.u32_le(next)?;
                        if s == DESCRIPTOR {
                            next = next.saturating_add(16);
                        } else if !matches!(s, LOCAL | CENTRAL | EOCD | EOCD64) {
                            next = next.saturating_add(12);
                        }
                    }
                    pos = next;
                }
            }
            CENTRAL => {
                centrals += 1;
                let nlen = u64::from(probe.u16_le(pos.saturating_add(28))?);
                let xlen = u64::from(probe.u16_le(pos.saturating_add(30))?);
                let clen = u64::from(probe.u16_le(pos.saturating_add(32))?);
                if names.len() < MAX_NAMES && nlen > 0 && nlen < 4096 {
                    let name =
                        probe.read(pos.saturating_add(46), usize::try_from(nlen).unwrap_or(0))?;
                    let name = String::from_utf8_lossy(&name).into_owned();
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                pos = pos
                    .saturating_add(46)
                    .saturating_add(nlen)
                    .saturating_add(xlen)
                    .saturating_add(clen);
            }
            EOCD64 => {
                let size = probe.u64_le(pos.saturating_add(4))?;
                pos = pos.saturating_add(12).saturating_add(size);
            }
            LOCATOR64 => {
                pos = pos.saturating_add(20);
            }
            DESCRIPTOR => {
                pos = pos.saturating_add(16);
            }
            EOCD => {
                let clen = u64::from(probe.u16_le(pos.saturating_add(20))?);
                let length = pos.saturating_add(22).saturating_add(clen) - start;
                checks.push(ValidationCheck::pass(
                    "entries",
                    format!(
                        "{locals} local header(s), {centrals} central directory entr{}{}",
                        if centrals == 1 { "y" } else { "ies" },
                        if descriptors_resolved > 0 {
                            format!(", {descriptors_resolved} data descriptor(s) resolved")
                        } else {
                            String::new()
                        }
                    ),
                ));
                checks.push(ValidationCheck::pass(
                    "end of central directory",
                    format!("record found; archive is {length} bytes"),
                ));
                let status = if centrals == 0 || centrals != locals {
                    ValidationStatus::MostlyValid
                } else {
                    ValidationStatus::Valid
                };
                let mut a =
                    Assembly::from_checks(length, true, std::mem::take(checks)).with_status(status);
                if let Some((id, name, ext)) = classify(&names) {
                    a.type_id = Some(id.into());
                    a.type_name = Some(name.into());
                    a.extension = Some(ext.into());
                }
                return Ok(Some(a));
            }
            other => {
                checks.push(ValidationCheck::fail(
                    "entries",
                    format!("unexpected signature {other:08X} at {} bytes in after {locals} entr{}: the archive is truncated or fragmented here",
                        pos - start, if locals == 1 { "y" } else { "ies" }),
                ));
                return Ok(Some(damaged(pos - start, checks, locals + centrals)));
            }
        }
    }
    checks.push(ValidationCheck::fail(
        "end of central directory",
        "not found within the size limit",
    ));
    Ok(Some(damaged(
        pos.min(end) - start,
        checks,
        locals + centrals,
    )))
}

/// Locates the data descriptor of a streamed entry whose data starts at
/// `data_start`: the next local/central signature preceded by a descriptor
/// whose compressed size matches the distance.
fn find_descriptor(
    probe: &mut Probe<'_>,
    data_start: u64,
    end: u64,
) -> Result<Option<(u64, u64)>, CarveError> {
    let mut from = data_start;
    for _ in 0..MAX_ENTRIES {
        let Some(pk) = probe.find(b"PK", from, end)? else {
            return Ok(None);
        };
        let sig = probe.u32_le(pk)?;
        if matches!(sig, LOCAL | CENTRAL) {
            // With signature: 16 bytes; without: 12 bytes.
            for (len, csize_at) in [(16u64, 8u64), (12, 4)] {
                if pk >= data_start.saturating_add(len) {
                    let desc = pk - len;
                    if len == 16 && probe.u32_le(desc)? != DESCRIPTOR {
                        continue;
                    }
                    let csize = u64::from(probe.u32_le(desc.saturating_add(csize_at))?);
                    if data_start.saturating_add(csize) == desc {
                        return Ok(Some((pk, len)));
                    }
                }
            }
        }
        from = pk.saturating_add(1);
    }
    Ok(None)
}

fn damaged(length: u64, checks: &mut Vec<ValidationCheck>, entries: usize) -> Assembly {
    Assembly::from_checks(length, false, std::mem::take(checks)).with_status(if entries == 0 {
        ValidationStatus::Invalid
    } else {
        ValidationStatus::Damaged
    })
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

    /// A stored (uncompressed) ZIP with the given entries; `descriptor`
    /// writes zero sizes in the local headers and a data descriptor after
    /// each entry, as streaming writers do.
    pub fn sample_zip(entries: &[(&str, &[u8])], descriptor: bool) -> Vec<u8> {
        let mut v = Vec::new();
        let mut central = Vec::new();
        for (name, data) in entries {
            let offset = v.len() as u32;
            let mut h = crc32fast::Hasher::new();
            h.update(data);
            let crc = h.finalize();
            v.extend_from_slice(&LOCAL.to_le_bytes());
            v.extend_from_slice(&[20, 0]);
            v.extend_from_slice(&(if descriptor { 8u16 } else { 0 }).to_le_bytes());
            v.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // method, time, date
            if descriptor {
                v.extend_from_slice(&[0u8; 12]);
            } else {
                v.extend_from_slice(&crc.to_le_bytes());
                v.extend_from_slice(&(data.len() as u32).to_le_bytes());
                v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            }
            v.extend_from_slice(&(name.len() as u16).to_le_bytes());
            v.extend_from_slice(&[0, 0]);
            v.extend_from_slice(name.as_bytes());
            v.extend_from_slice(data);
            if descriptor {
                v.extend_from_slice(&DESCRIPTOR.to_le_bytes());
                v.extend_from_slice(&crc.to_le_bytes());
                v.extend_from_slice(&(data.len() as u32).to_le_bytes());
                v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            }
            central.extend_from_slice(&CENTRAL.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0]);
            central.extend_from_slice(&(if descriptor { 8u16 } else { 0 }).to_le_bytes());
            central.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0u8; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = v.len() as u32;
        v.extend_from_slice(&central);
        v.extend_from_slice(&EOCD.to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        v.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        v.extend_from_slice(&(central.len() as u32).to_le_bytes());
        v.extend_from_slice(&cd_offset.to_le_bytes());
        v.extend_from_slice(&[0, 0]);
        v
    }

    #[test]
    fn walks_plain_and_streamed_archives() {
        let plain = sample_zip(&[("a.txt", b"hello"), ("dir/b.bin", &[1u8; 700])], false);
        let r = run(&ZipAssembler, &plain, b"PK\x03\x04 next archive").unwrap();
        assert_eq!(r.length, plain.len() as u64, "{:?}", r.checks);
        assert_eq!(r.status, ValidationStatus::Valid);
        let streamed = sample_zip(
            &[
                ("[Content_Types].xml", b"<Types/>"),
                ("word/document.xml", b"<w:document/>"),
            ],
            true,
        );
        let r = run(&ZipAssembler, &streamed, b"PK\x03\x04").unwrap();
        assert_eq!(r.length, streamed.len() as u64, "{:?}", r.checks);
        assert_eq!(r.type_id.as_deref(), Some("docx"));
        assert_eq!(r.extension.as_deref(), Some("docx"));
        // Truncated inside the second entry: damaged, open-ended.
        let cut = &plain[..200];
        let r = run(&ZipAssembler, cut, &[0x55u8; 300]).unwrap();
        assert!(!r.end_known);
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(run(&ZipAssembler, b"PK\x05\x06", b"").is_none());
    }

    #[test]
    fn zip64_extra_field() {
        let mut extra = vec![0x01, 0x00, 16, 0];
        extra.extend_from_slice(&5_000_000_000u64.to_le_bytes());
        extra.extend_from_slice(&4_000_000_000u64.to_le_bytes());
        assert_eq!(
            zip64_sizes(&extra, u32::MAX, u32::MAX),
            Some((4_000_000_000, 5_000_000_000))
        );
        assert_eq!(zip64_sizes(&extra, 7, u32::MAX), Some((7, 5_000_000_000)));
        assert_eq!(zip64_sizes(&[0x02, 0x00, 0, 0], 1, 2), None);
    }
}
