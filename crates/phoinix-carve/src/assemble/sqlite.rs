//! SQLite: page size × page count from the 100-byte header.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, be16, be32, clamp_len};
use crate::CarveError;
use crate::probe::Probe;

/// SQLite assembler.
pub struct SqliteAssembler;

impl Assembler for SqliteAssembler {
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError> {
        let head = probe.read_available(start, 100)?;
        if head.get(..16) != Some(b"SQLite format 3\0") || head.len() < 100 {
            return Ok(None);
        }
        let page_size = match be16(&head, 16).unwrap_or(0) {
            1 => 65_536u64,
            n if n >= 512 && n.is_power_of_two() => u64::from(n),
            _ => return Ok(None),
        };
        let pages = u64::from(be32(&head, 28).unwrap_or(0));
        let change_counter = be32(&head, 24).unwrap_or(0);
        let valid_for = be32(&head, 92).unwrap_or(1);
        let fractions = (
            head.get(21).copied().unwrap_or(0),
            head.get(22).copied().unwrap_or(0),
            head.get(23).copied().unwrap_or(0),
        );
        let mut checks = vec![
            ValidationCheck::pass("header", "SQLite format 3 header"),
            ValidationCheck::pass("page size", format!("{page_size} bytes")),
        ];
        if fractions != (64, 32, 32) {
            checks.push(ValidationCheck::fail(
                "payload fractions",
                "reserved header bytes have unexpected values",
            ));
            return Ok(Some(
                Assembly::from_checks(100, false, checks).with_status(ValidationStatus::Invalid),
            ));
        }
        checks.push(ValidationCheck::pass("payload fractions", "64/32/32"));
        if pages == 0 {
            checks.push(ValidationCheck::fail(
                "page count",
                "header declares zero pages (legacy writer)",
            ));
            let length = clamp_len(start, max_len, max_len, probe.limit());
            return Ok(Some(
                Assembly::from_checks(length, false, checks).with_status(ValidationStatus::Damaged),
            ));
        }
        let size = page_size.saturating_mul(pages);
        if change_counter == valid_for {
            checks.push(ValidationCheck::pass(
                "page count",
                format!("{pages} pages (version-valid-for matches the change counter)"),
            ));
        } else {
            checks.push(ValidationCheck::fail("page count", format!("{pages} pages declared, but the count may be stale (version-valid-for differs from the change counter)")));
        }
        let length = clamp_len(start, size, max_len, probe.limit());
        if length < size {
            checks.push(ValidationCheck::fail(
                "declared size",
                format!("{size} bytes declared, only {length} readable"),
            ));
            return Ok(Some(
                Assembly::from_checks(length, false, checks).with_status(ValidationStatus::Damaged),
            ));
        }
        checks.push(ValidationCheck::pass(
            "declared size",
            format!("{size} bytes"),
        ));
        Ok(Some(Assembly::from_checks(length, true, checks)))
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

    pub fn sample_sqlite(pages: u32) -> Vec<u8> {
        let mut v = vec![0u8; 1024 * pages as usize];
        v[..16].copy_from_slice(b"SQLite format 3\0");
        v[16..18].copy_from_slice(&1024u16.to_be_bytes());
        v[18] = 1;
        v[19] = 1;
        v[21] = 64;
        v[22] = 32;
        v[23] = 32;
        v[24..28].copy_from_slice(&7u32.to_be_bytes());
        v[28..32].copy_from_slice(&pages.to_be_bytes());
        v[92..96].copy_from_slice(&7u32.to_be_bytes());
        v[96..100].copy_from_slice(&3_046_000u32.to_be_bytes());
        v
    }

    #[test]
    fn declared_pages() {
        let db = sample_sqlite(3);
        let r = run(&SqliteAssembler, &db, b"x").unwrap();
        assert_eq!(r.length, 3072);
        assert_eq!(r.status, ValidationStatus::Valid, "{:?}", r.checks);
        let r = run(&SqliteAssembler, &db[..2048], b"").unwrap();
        assert_eq!(r.status, ValidationStatus::Damaged);
        let mut bad = db.clone();
        bad[21] = 0;
        assert_eq!(
            run(&SqliteAssembler, &bad, b"").unwrap().status,
            ValidationStatus::Invalid
        );
    }
}
