//! Generic assemblers for declarative signatures.

use phoinix_health::{ValidationCheck, ValidationStatus};

use super::{Assembler, Assembly, clamp_len};
use crate::CarveError;
use crate::probe::{Find, Probe};

/// Header plus footer search.
pub struct FooterAssembler {
    /// Footer bytes.
    pub footer: Vec<u8>,
    /// Bytes occupied by the header patterns.
    pub header_span: u64,
}

impl Assembler for FooterAssembler {
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError> {
        let end = start.saturating_add(max_len);
        let from = start.saturating_add(self.header_span);
        // A whole window of zeros before the footer means the file's data
        // gave way to free or discarded space: stop there instead of
        // walking to the size limit.
        match probe.find_bounded(&self.footer, from, end, &|w| w.iter().all(|b| *b == 0))? {
            Find::Found(pos) => {
                let length = pos.saturating_add(self.footer.len() as u64) - start;
                Ok(Some(Assembly::from_checks(
                    length,
                    true,
                    vec![
                        ValidationCheck::pass("header", "signature present"),
                        ValidationCheck::pass("footer", format!("footer found {length} bytes in")),
                    ],
                )))
            }
            Find::GaveUp(at) => {
                let length = at - start;
                Ok(Some(
                    Assembly::from_checks(
                        length,
                        false,
                        vec![
                            ValidationCheck::pass("header", "signature present"),
                            ValidationCheck::fail(
                                "footer",
                                format!("the data becomes zero-filled {length} bytes in, before any footer: the file ends there"),
                            ),
                        ],
                    )
                    .with_status(ValidationStatus::Damaged),
                ))
            }
            Find::Exhausted => {
                let length = clamp_len(start, max_len, max_len, probe.limit());
                Ok(Some(
                    Assembly::from_checks(
                        length,
                        false,
                        vec![
                            ValidationCheck::pass("header", "signature present"),
                            ValidationCheck::fail(
                                "footer",
                                "footer not found within the size limit",
                            ),
                        ],
                    )
                    .with_status(ValidationStatus::Damaged),
                ))
            }
        }
    }
}

/// Header only: the file extends to the size limit.
pub struct HeaderOnlyAssembler;

impl Assembler for HeaderOnlyAssembler {
    fn assemble(
        &self,
        probe: &mut Probe<'_>,
        start: u64,
        max_len: u64,
    ) -> Result<Option<Assembly>, CarveError> {
        let length = clamp_len(start, max_len, max_len, probe.limit());
        Ok(Some(
            Assembly::from_checks(
                length,
                false,
                vec![ValidationCheck::pass("header", "signature present")],
            )
            .with_status(ValidationStatus::Unknown),
        ))
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation
    )]
    use super::super::testutil::run;
    use super::*;

    #[test]
    fn footer_search() {
        let a = FooterAssembler {
            footer: b"END".to_vec(),
            header_span: 3,
        };
        let r = run(&a, b"FOO hello END", b"trailing").unwrap();
        assert_eq!(r.length, 13);
        assert!(r.end_known);
        assert_eq!(r.status, ValidationStatus::Valid);
        let r = run(&a, b"FOO hello", b"").unwrap();
        assert!(!r.end_known);
        assert_eq!(r.status, ValidationStatus::Damaged);
        let r = run(&HeaderOnlyAssembler, b"FOO", b"x").unwrap();
        assert_eq!(r.status, ValidationStatus::Unknown);
        assert_eq!(r.length, 4);
    }

    #[test]
    fn footer_search_gives_up_on_zero_filled_data() {
        let a = FooterAssembler {
            footer: b"END".to_vec(),
            header_span: 3,
        };
        // A header followed by far more zeros than one probe window, and a
        // footer only after them: the file ends where the zeros begin.
        let mut tail = vec![0u8; 3 * crate::probe::WINDOW_BYTES];
        tail.extend_from_slice(b"END");
        let r = run(&a, b"FOO hello", &tail).unwrap();
        assert!(!r.end_known);
        assert_eq!(r.status, ValidationStatus::Damaged);
        assert!(
            r.length < 2 * crate::probe::WINDOW_BYTES as u64,
            "{}",
            r.length
        );
    }
}
