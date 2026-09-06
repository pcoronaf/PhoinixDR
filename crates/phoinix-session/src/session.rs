//! Scan sessions: the candidates of one scan with their evidence, stored as
//! a JSON `.phx` file. Only metadata and evidence are stored, never
//! recovered content.

use std::path::{Path, PathBuf};

use phoinix_carve::CarveReport;
use phoinix_core::{CandidateId, FileSystemType};
use phoinix_fs::RecoveryCandidate;
use phoinix_health::CandidateSource;
use serde::{Deserialize, Serialize};

use crate::SessionError;
use crate::dto::{CandidateSummary, ScanMode, SessionSummary, VolumeInfo};

/// File format version.
pub const FORMAT_VERSION: u32 = 1;
/// File extension of session files.
pub const EXTENSION: &str = "phx";

/// A scan session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScanSession {
    /// Format version.
    pub version: u32,
    /// Session identifier (UUID text).
    pub id: String,
    /// Source path.
    pub source: PathBuf,
    /// A human label for a device source (model and serial number), so
    /// sessions from different drives that had the same device path can be
    /// told apart. `None` for images and unknown devices.
    #[serde(default)]
    pub source_label: Option<String>,
    /// The scanned volume.
    pub volume: VolumeInfo,
    /// Quick or deep.
    pub mode: ScanMode,
    /// Unix seconds when the scan started.
    pub started: i64,
    /// Unix seconds when the scan finished.
    pub finished: Option<i64>,
    /// Whether the scan ran to completion.
    pub complete: bool,
    /// Carving statistics.
    pub carving: Option<CarveReport>,
    /// The image container the source was, when it is an image.
    #[serde(default)]
    pub container: Option<phoinix_image::ContainerInfo>,
    /// The candidates with their full evidence.
    pub candidates: Vec<RecoveryCandidate>,
    /// Where the session was last saved.
    #[serde(skip)]
    pub file: Option<PathBuf>,
}

/// Current Unix time in seconds.
#[must_use]
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

impl ScanSession {
    /// A new, empty session.
    #[must_use]
    pub fn new(source: PathBuf, volume: VolumeInfo, mode: ScanMode) -> Self {
        Self {
            version: FORMAT_VERSION,
            id: uuid_text(),
            source,
            source_label: None,
            volume,
            mode,
            started: now(),
            finished: None,
            complete: false,
            carving: None,
            container: None,
            candidates: Vec::new(),
            file: None,
        }
    }

    /// The filesystem of the volume.
    #[must_use]
    pub const fn filesystem(&self) -> FileSystemType {
        self.volume.filesystem
    }

    /// Finds a candidate by id.
    #[must_use]
    pub fn candidate(&self, id: CandidateId) -> Option<&RecoveryCandidate> {
        self.candidates.iter().find(|c| c.id == id)
    }

    /// Row summaries of every candidate.
    #[must_use]
    pub fn summaries(&self) -> Vec<CandidateSummary> {
        self.candidates
            .iter()
            .map(CandidateSummary::from_candidate)
            .collect()
    }

    /// Summary of the session.
    #[must_use]
    pub fn summary(&self) -> SessionSummary {
        let carved = self
            .candidates
            .iter()
            .filter(|c| c.evidence.source == CandidateSource::FileCarving)
            .count();
        SessionSummary {
            id: self.id.clone(),
            file: self.file.clone(),
            source: self.source.clone(),
            source_label: self.source_label.clone(),
            partition: self.volume.partition,
            filesystem: self.volume.filesystem,
            mode: self.mode,
            started: self.started,
            finished: self.finished,
            complete: self.complete,
            candidates: self.candidates.len(),
            from_metadata: self.candidates.len() - carved,
            carved,
            carving: self.carving,
        }
    }

    /// Saves the session as JSON to `path`.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Io`] on write failures.
    pub fn save(&mut self, path: &Path) -> Result<(), SessionError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|e| SessionError::io(format!("creating {}", parent.display()), e))?;
        }
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| SessionError::InvalidSession(e.to_string()))?;
        let tmp = path.with_extension("phx.tmp");
        std::fs::write(&tmp, json)
            .map_err(|e| SessionError::io(format!("writing {}", tmp.display()), e))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| SessionError::io(format!("renaming to {}", path.display()), e))?;
        self.file = Some(path.to_path_buf());
        Ok(())
    }

    /// Loads a session from `path`.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Io`] or [`SessionError::InvalidSession`].
    pub fn load(path: &Path) -> Result<Self, SessionError> {
        let text = std::fs::read(path)
            .map_err(|e| SessionError::io(format!("reading {}", path.display()), e))?;
        let mut session: Self = serde_json::from_slice(&text)
            .map_err(|e| SessionError::InvalidSession(e.to_string()))?;
        if session.version > FORMAT_VERSION {
            return Err(SessionError::InvalidSession(format!(
                "format version {} is newer than supported ({FORMAT_VERSION})",
                session.version
            )));
        }
        session.file = Some(path.to_path_buf());
        Ok(session)
    }

    /// Reads only the summary of a session file.
    ///
    /// # Errors
    ///
    /// See [`load`](Self::load).
    pub fn load_summary(path: &Path) -> Result<SessionSummary, SessionError> {
        Ok(Self::load(path)?.summary())
    }

    /// A file name for this session (`<started>-<id>.phx`).
    #[must_use]
    pub fn default_file_name(&self) -> String {
        format!(
            "{}-{}.{EXTENSION}",
            self.started,
            &self.id[..8.min(self.id.len())]
        )
    }
}

fn uuid_text() -> String {
    CandidateId::new().to_string()
}
