//! Recovery health: likelihood, confidence, category and reasons.

use std::fmt;

use serde::{Deserialize, Serialize};

/// User-facing category derived from the likelihood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCategory {
    /// 0.
    Unrecoverable,
    /// 1–34.
    VeryPoor,
    /// 35–59.
    Poor,
    /// 60–79.
    Good,
    /// 80–94.
    VeryGood,
    /// 95–100.
    Excellent,
    /// Not assessable.
    Unknown,
}

impl HealthCategory {
    /// Maps a likelihood to a category (provisional thresholds).
    #[must_use]
    pub const fn from_likelihood(likelihood: u8) -> Self {
        match likelihood {
            0 => Self::Unrecoverable,
            1..=34 => Self::VeryPoor,
            35..=59 => Self::Poor,
            60..=79 => Self::Good,
            80..=94 => Self::VeryGood,
            _ => Self::Excellent,
        }
    }

    /// User-facing label.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Excellent => "Excellent",
            Self::VeryGood => "Very good",
            Self::Good => "Good",
            Self::Poor => "Poor",
            Self::VeryPoor => "Very poor",
            Self::Unrecoverable => "Unrecoverable",
            Self::Unknown => "Unknown",
        }
    }

    /// Parses a label as typed on a command line (case-insensitive, spaces,
    /// hyphens or underscores allowed).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let key: String = text
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        Some(match key.as_str() {
            "excellent" => Self::Excellent,
            "verygood" => Self::VeryGood,
            "good" => Self::Good,
            "poor" => Self::Poor,
            "verypoor" => Self::VeryPoor,
            "unrecoverable" => Self::Unrecoverable,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

impl fmt::Display for HealthCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// One reason behind a score.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReason {
    /// Whether this supports recovery (`✓`) or counts against it (`⚠`).
    pub positive: bool,
    /// Human-readable text.
    pub text: String,
}

impl HealthReason {
    /// A supporting reason.
    #[must_use]
    pub fn positive(text: impl Into<String>) -> Self {
        Self {
            positive: true,
            text: text.into(),
        }
    }

    /// A detracting reason.
    #[must_use]
    pub fn negative(text: impl Into<String>) -> Self {
        Self {
            positive: false,
            text: text.into(),
        }
    }
}

/// The assessment of a candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryHealth {
    /// Estimated probability (0–100) that recovery yields the original
    /// content.
    pub likelihood: u8,
    /// Quality (0–100) of the evidence behind the estimate.
    pub confidence: u8,
    /// Category derived from the likelihood.
    pub category: HealthCategory,
    /// Concrete reasons, positive first.
    pub reasons: Vec<HealthReason>,
}

impl RecoveryHealth {
    /// A health value for candidates that cannot be assessed.
    #[must_use]
    pub fn unknown(reason: impl Into<String>) -> Self {
        Self {
            likelihood: 0,
            confidence: 0,
            category: HealthCategory::Unknown,
            reasons: vec![HealthReason::negative(reason)],
        }
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
    use super::*;

    #[test]
    fn category_thresholds() {
        assert_eq!(
            HealthCategory::from_likelihood(0),
            HealthCategory::Unrecoverable
        );
        assert_eq!(HealthCategory::from_likelihood(1), HealthCategory::VeryPoor);
        assert_eq!(
            HealthCategory::from_likelihood(34),
            HealthCategory::VeryPoor
        );
        assert_eq!(HealthCategory::from_likelihood(35), HealthCategory::Poor);
        assert_eq!(HealthCategory::from_likelihood(60), HealthCategory::Good);
        assert_eq!(
            HealthCategory::from_likelihood(80),
            HealthCategory::VeryGood
        );
        assert_eq!(
            HealthCategory::from_likelihood(95),
            HealthCategory::Excellent
        );
        assert_eq!(
            HealthCategory::from_likelihood(100),
            HealthCategory::Excellent
        );
        assert_eq!(
            HealthCategory::parse("very-good"),
            Some(HealthCategory::VeryGood)
        );
        assert_eq!(
            HealthCategory::parse("Very Poor"),
            Some(HealthCategory::VeryPoor)
        );
        assert_eq!(HealthCategory::parse("nope"), None);
    }
}
