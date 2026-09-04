//! Filesystem probing.

use phoinix_block::BlockReader;
use phoinix_core::FileSystemType;
use serde::{Deserialize, Serialize};

use crate::FsError;

/// Confidence at or above which a probe result is considered a positive
/// identification.
pub const POSITIVE_THRESHOLD: u8 = 50;

/// One piece of evidence gathered by a probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeEvidence {
    /// Whether the evidence supports (`true`) or contradicts (`false`) the
    /// identification.
    pub supports: bool,
    /// Human-readable description.
    pub description: String,
}

impl ProbeEvidence {
    /// Supporting evidence.
    #[must_use]
    pub fn supports(description: impl Into<String>) -> Self {
        Self {
            supports: true,
            description: description.into(),
        }
    }

    /// Contradicting evidence.
    #[must_use]
    pub fn contradicts(description: impl Into<String>) -> Self {
        Self {
            supports: false,
            description: description.into(),
        }
    }
}

/// What a probe concluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    /// The filesystem this probe looks for.
    pub filesystem: FileSystemType,
    /// Confidence 0–100 that the source holds that filesystem.
    pub confidence: u8,
    /// The evidence behind the confidence.
    pub evidence: Vec<ProbeEvidence>,
}

impl ProbeResult {
    /// A result for "definitely not this filesystem".
    #[must_use]
    pub fn negative(filesystem: FileSystemType, reason: impl Into<String>) -> Self {
        Self {
            filesystem,
            confidence: 0,
            evidence: vec![ProbeEvidence::contradicts(reason)],
        }
    }

    /// Whether the confidence reaches [`POSITIVE_THRESHOLD`].
    #[must_use]
    pub const fn is_positive(&self) -> bool {
        self.confidence >= POSITIVE_THRESHOLD
    }
}

/// Recognises one filesystem family.
pub trait FileSystemProbe: Send + Sync {
    /// The filesystem this probe detects.
    fn filesystem(&self) -> FileSystemType;

    /// Examines `reader` (a volume, not a whole disk) and reports evidence.
    ///
    /// Probes must never read outside the source and must return a result
    /// (typically negative) rather than an error for unrecognised content;
    /// errors are reserved for I/O failures.
    ///
    /// # Errors
    ///
    /// Returns [`FsError`] for I/O failures.
    fn probe(&self, reader: &dyn BlockReader) -> Result<ProbeResult, FsError>;
}

/// The outcome of running every registered probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Detection {
    /// Best positive result, if any.
    pub best: Option<ProbeResult>,
    /// All results, most confident first.
    pub results: Vec<ProbeResult>,
}

impl Detection {
    /// The identified filesystem type, or [`FileSystemType::Unknown`].
    #[must_use]
    pub fn filesystem(&self) -> FileSystemType {
        self.best
            .as_ref()
            .map_or(FileSystemType::Unknown, |b| b.filesystem)
    }
}

/// A collection of probes.
#[derive(Default)]
pub struct ProbeRegistry {
    probes: Vec<Box<dyn FileSystemProbe>>,
}

impl ProbeRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a probe.
    #[must_use]
    pub fn with(mut self, probe: Box<dyn FileSystemProbe>) -> Self {
        self.probes.push(probe);
        self
    }

    /// Adds a probe in place.
    pub fn register(&mut self, probe: Box<dyn FileSystemProbe>) {
        self.probes.push(probe);
    }

    /// Number of registered probes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.probes.len()
    }

    /// Whether no probes are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.probes.is_empty()
    }

    /// Runs every probe and ranks the results.
    ///
    /// A probe that fails with an I/O error is logged and skipped so that one
    /// unreadable region cannot hide other evidence.
    #[must_use]
    pub fn detect(&self, reader: &dyn BlockReader) -> Detection {
        let mut results: Vec<ProbeResult> = Vec::new();
        for probe in &self.probes {
            match probe.probe(reader) {
                Ok(result) => results.push(result),
                Err(err) => {
                    tracing::warn!(filesystem = %probe.filesystem(), error = %err, "probe failed")
                }
            }
        }
        results.sort_by_key(|a| std::cmp::Reverse(a.confidence));
        let best = results.first().filter(|r| r.is_positive()).cloned();
        Detection { best, results }
    }
}

impl std::fmt::Debug for ProbeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<String> = self
            .probes
            .iter()
            .map(|p| p.filesystem().to_string())
            .collect();
        f.debug_struct("ProbeRegistry")
            .field("probes", &names)
            .finish()
    }
}
