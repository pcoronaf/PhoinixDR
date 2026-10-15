//! Post-erase verification scans (milestone M12.2): after the passes and
//! the read-back, the recovery engines look at the target the way a
//! recovery attempt would. The scans themselves run in the session layer
//! (the engines and the carver live there); this module defines what the
//! erase engine asks for and what the certificate records.

use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::target::TargetDescription;

/// Which scan to run after the erase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PostScan {
    /// No scan.
    #[default]
    None,
    /// Filesystem metadata: deleted records and whether their content
    /// survived.
    Quick,
    /// Metadata plus signature carving of the erased space.
    Deep,
}

impl PostScan {
    /// Parses `none`, `quick` or `deep`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "none" => Some(Self::None),
            "quick" => Some(Self::Quick),
            "deep" => Some(Self::Deep),
            _ => None,
        }
    }

    /// The identifier.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Quick => "quick",
            Self::Deep => "deep",
        }
    }
}

/// What the post-erase scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanRecord {
    /// The scan requested.
    pub mode: PostScan,
    /// Whether it could run (a mounted volume may not be readable raw
    /// without administrator rights; the note says why not).
    pub ran: bool,
    /// Why it did not run, or what it looked at.
    pub note: Option<String>,
    /// Filesystem the scan found on the target, if any ("none" after a
    /// whole-partition overwrite is the expected outcome).
    pub filesystem: Option<String>,
    /// Partition scheme found on a whole-device target, if any.
    pub partition_table: Option<String>,
    /// Deleted records the metadata walk found.
    pub candidates: u64,
    /// Of those, the ones whose content was examined and was not entirely
    /// zero: data a recovery would still return.
    pub with_content: u64,
    /// Files found by carving the erased space (deep scan).
    pub carved: u64,
    /// Bytes carved (deep scan).
    pub bytes_scanned: u64,
    /// Seconds spent.
    pub seconds: u64,
    /// Whether the scan was cut short.
    pub cancelled: bool,
    /// `true` when the scan ran to the end and found nothing recoverable.
    pub passed: bool,
}

impl ScanRecord {
    /// A record for a scan that could not run.
    #[must_use]
    pub fn not_run(mode: PostScan, why: impl Into<String>) -> Self {
        Self {
            mode,
            ran: false,
            note: Some(why.into()),
            ..Default::default()
        }
    }

    /// One-line summary for the verdict.
    #[must_use]
    pub fn summary(&self) -> String {
        if !self.ran {
            return format!(
                "post-erase scan not run{}",
                self.note
                    .as_deref()
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default()
            );
        }
        let what = match self.mode {
            PostScan::Deep => "deep scan",
            _ => "quick scan",
        };
        if self.cancelled {
            return format!("post-erase {what} cancelled");
        }
        if self.passed {
            format!("post-erase {what} found nothing recoverable")
        } else {
            format!(
                "POST-ERASE {} FOUND {} candidate(s) with content and {} carved file(s)",
                what.to_ascii_uppercase(),
                self.with_content,
                self.carved
            )
        }
    }
}

/// Progress of a post-erase scan, forwarded as an erase event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanProgress {
    /// Phase name (`opening`, `metadata`, `carving`, `assembling`).
    pub phase: String,
    /// Units done.
    pub done: u64,
    /// Units in total, when known.
    pub total: Option<u64>,
    /// Candidates so far.
    pub candidates: u64,
}

/// Runs the post-erase scan. Implemented by the session layer.
pub trait PostScanner: Send + Sync {
    /// Scans the erased target; `progress` receives phase progress and
    /// `cancel` cuts the scan short.
    fn scan(
        &self,
        target: &TargetDescription,
        mode: PostScan,
        progress: &mut dyn FnMut(ScanProgress),
        cancel: &AtomicBool,
    ) -> ScanRecord;
}
