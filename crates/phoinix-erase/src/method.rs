//! Erase methods: what each pass writes, and what the method is worth.

use serde::{Deserialize, Serialize};

/// What one pass writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PassPattern {
    /// Every byte the same.
    Fixed {
        /// The byte.
        byte: u8,
    },
    /// A repeating byte sequence.
    Sequence {
        /// The bytes, repeated.
        bytes: Vec<u8>,
    },
    /// Cryptographically random data, reproducible from the run's seed so
    /// that read-back verification can compare it.
    Random,
}

impl PassPattern {
    /// Short label (`0x00`, `random`, `0x92 0x49 0x24`).
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Fixed { byte } => format!("0x{byte:02X}"),
            Self::Sequence { bytes } => bytes
                .iter()
                .map(|b| format!("0x{b:02X}"))
                .collect::<Vec<_>>()
                .join(" "),
            Self::Random => "random".to_owned(),
        }
    }
}

/// One pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pass {
    /// What is written.
    pub pattern: PassPattern,
}

impl Pass {
    const fn fixed(byte: u8) -> Self {
        Self {
            pattern: PassPattern::Fixed { byte },
        }
    }
    const fn random() -> Self {
        Self {
            pattern: PassPattern::Random,
        }
    }
    fn seq(bytes: &[u8]) -> Self {
        Self {
            pattern: PassPattern::Sequence {
                bytes: bytes.to_vec(),
            },
        }
    }
}

/// How the result is checked after the last pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    /// No read-back.
    None,
    /// A sample of blocks spread over the target (every block for small
    /// targets).
    #[default]
    Sample,
    /// Every byte.
    Full,
}

/// An erase method: a named sequence of passes with its guidance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "id", rename_all = "kebab-case")]
pub enum EraseMethod {
    /// One pass of zeros (NIST 800-88 Clear).
    Zero,
    /// One pass of random data (NIST 800-88 Clear).
    Random,
    /// One random pass plus the verification 800-88 asks for.
    Nist,
    /// DoD 5220.22-M (E): 0x00, 0xFF, random; obsolete since 2007.
    Dod3,
    /// DoD 5220.22-M (ECE): seven passes; obsolete since 2007.
    Dod7,
    /// Schneier: 0xFF, 0x00, five random passes.
    Schneier,
    /// BSI VSITR (1999): seven passes.
    Vsitr,
    /// Gutmann (1996): 35 passes.
    Gutmann,
    /// A user-defined sequence.
    Custom {
        /// The passes.
        passes: Vec<Pass>,
    },
}

/// Gutmann's 27 fixed patterns (passes 5 to 31), as 3-byte sequences.
const GUTMANN_FIXED: [[u8; 3]; 27] = [
    [0x55, 0x55, 0x55],
    [0xAA, 0xAA, 0xAA],
    [0x92, 0x49, 0x24],
    [0x49, 0x24, 0x92],
    [0x24, 0x92, 0x49],
    [0x00, 0x00, 0x00],
    [0x11, 0x11, 0x11],
    [0x22, 0x22, 0x22],
    [0x33, 0x33, 0x33],
    [0x44, 0x44, 0x44],
    [0x55, 0x55, 0x55],
    [0x66, 0x66, 0x66],
    [0x77, 0x77, 0x77],
    [0x88, 0x88, 0x88],
    [0x99, 0x99, 0x99],
    [0xAA, 0xAA, 0xAA],
    [0xBB, 0xBB, 0xBB],
    [0xCC, 0xCC, 0xCC],
    [0xDD, 0xDD, 0xDD],
    [0xEE, 0xEE, 0xEE],
    [0xFF, 0xFF, 0xFF],
    [0x92, 0x49, 0x24],
    [0x49, 0x24, 0x92],
    [0x24, 0x92, 0x49],
    [0x6D, 0xB6, 0xDB],
    [0xB6, 0xDB, 0x6D],
    [0xDB, 0x6D, 0xB6],
];

impl EraseMethod {
    /// Every built-in method, in the order the interface lists them.
    #[must_use]
    pub fn builtin() -> Vec<Self> {
        vec![
            Self::Zero,
            Self::Random,
            Self::Nist,
            Self::Dod3,
            Self::Dod7,
            Self::Schneier,
            Self::Vsitr,
            Self::Gutmann,
        ]
    }

    /// Parses a method id (`zero`, `dod-3`, `gutmann`, ...).
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        Self::builtin()
            .into_iter()
            .find(|m| m.id() == id.trim().to_ascii_lowercase())
    }

    /// The identifier.
    #[must_use]
    pub fn id(&self) -> String {
        match self {
            Self::Zero => "zero".into(),
            Self::Random => "random".into(),
            Self::Nist => "nist".into(),
            Self::Dod3 => "dod-3".into(),
            Self::Dod7 => "dod-7".into(),
            Self::Schneier => "schneier".into(),
            Self::Vsitr => "vsitr".into(),
            Self::Gutmann => "gutmann".into(),
            Self::Custom { .. } => "custom".into(),
        }
    }

    /// Human-readable name.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Zero => "Zeros, one pass".into(),
            Self::Random => "Random data, one pass".into(),
            Self::Nist => "NIST SP 800-88 Clear (random, verified)".into(),
            Self::Dod3 => "DoD 5220.22-M (E), 3 passes".into(),
            Self::Dod7 => "DoD 5220.22-M (ECE), 7 passes".into(),
            Self::Schneier => "Schneier, 7 passes".into(),
            Self::Vsitr => "VSITR (BSI), 7 passes".into(),
            Self::Gutmann => "Gutmann, 35 passes".into(),
            Self::Custom { passes } => format!("Custom, {} pass(es)", passes.len()),
        }
    }

    /// What the user should know about the method.
    #[must_use]
    pub fn note(&self) -> &'static str {
        match self {
            Self::Zero => {
                "NIST 800-88 Clear. The fastest method and the default for magnetic disks; one pass is enough on any drive made this century."
            }
            Self::Random => {
                "NIST 800-88 Clear. Leaves no readable pattern, so an erased disk cannot be told from an encrypted one."
            }
            Self::Nist => {
                "The documented Clear procedure: one random pass followed by the read-back verification SP 800-88 asks for."
            }
            Self::Dod3 => {
                "The DoD 5220.22-M three-pass sequence (0x00, 0xFF, random). Removed from the NISPOM in 2007; kept because policies still name it."
            }
            Self::Dod7 => {
                "The DoD 5220.22-M ECE seven-pass sequence. Obsolete since 2007; kept because policies still name it."
            }
            Self::Schneier => {
                "0xFF, 0x00 and five random passes, from Applied Cryptography (1996)."
            }
            Self::Vsitr => {
                "The German BSI 1999 sequence: alternating 0x00/0xFF three times, then 0xAA."
            }
            Self::Gutmann => {
                "Peter Gutmann's 1996 sequence for MFM/RLL encodings. The author's epilogue: \u{201c}performing the full 35-pass overwrite is pointless for any drive\u{201d}."
            }
            Self::Custom { .. } => "A user-defined sequence of passes.",
        }
    }

    /// The passes, in order.
    #[must_use]
    pub fn passes(&self) -> Vec<Pass> {
        match self {
            Self::Zero => vec![Pass::fixed(0x00)],
            Self::Random | Self::Nist => vec![Pass::random()],
            Self::Dod3 => vec![Pass::fixed(0x00), Pass::fixed(0xFF), Pass::random()],
            Self::Dod7 => vec![
                Pass::fixed(0x00),
                Pass::fixed(0xFF),
                Pass::random(),
                Pass::fixed(0x00),
                Pass::fixed(0x00),
                Pass::fixed(0xFF),
                Pass::random(),
            ],
            Self::Schneier => {
                let mut v = vec![Pass::fixed(0xFF), Pass::fixed(0x00)];
                v.extend(std::iter::repeat_n(Pass::random(), 5));
                v
            }
            Self::Vsitr => vec![
                Pass::fixed(0x00),
                Pass::fixed(0xFF),
                Pass::fixed(0x00),
                Pass::fixed(0xFF),
                Pass::fixed(0x00),
                Pass::fixed(0xFF),
                Pass::fixed(0xAA),
            ],
            Self::Gutmann => {
                let mut v: Vec<Pass> = std::iter::repeat_n(Pass::random(), 4).collect();
                v.extend(GUTMANN_FIXED.iter().map(|b| Pass::seq(b)));
                v.extend(std::iter::repeat_n(Pass::random(), 4));
                v
            }
            Self::Custom { passes } => passes.clone(),
        }
    }

    /// Number of passes.
    #[must_use]
    pub fn pass_count(&self) -> usize {
        self.passes().len()
    }

    /// Whether the multi-pass warning applies.
    #[must_use]
    pub fn multi_pass(&self) -> bool {
        self.pass_count() > 1
    }

    /// Whether the method implies read-back verification by itself.
    #[must_use]
    pub fn implies_verification(&self) -> bool {
        matches!(self, Self::Nist)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    #[test]
    fn pass_counts_match_the_published_schemes() {
        assert_eq!(EraseMethod::Zero.pass_count(), 1);
        assert_eq!(EraseMethod::Random.pass_count(), 1);
        assert_eq!(EraseMethod::Dod3.pass_count(), 3);
        assert_eq!(EraseMethod::Dod7.pass_count(), 7);
        assert_eq!(EraseMethod::Schneier.pass_count(), 7);
        assert_eq!(EraseMethod::Vsitr.pass_count(), 7);
        assert_eq!(EraseMethod::Gutmann.pass_count(), 35);
        assert!(!EraseMethod::Zero.multi_pass());
        assert!(EraseMethod::Gutmann.multi_pass());
        assert_eq!(EraseMethod::parse("DoD-3"), Some(EraseMethod::Dod3));
        assert_eq!(EraseMethod::parse("nope"), None);
        for m in EraseMethod::builtin() {
            assert_eq!(EraseMethod::parse(&m.id()), Some(m.clone()));
            assert!(!m.note().is_empty());
        }
        assert_eq!(
            EraseMethod::Gutmann.passes()[6].pattern.label(),
            "0x92 0x49 0x24"
        );
    }
}
