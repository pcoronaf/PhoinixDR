//! The workspace: what a front-end holds. Sessions directory, device list,
//! background scans with an event channel, recovery and previews.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use phoinix_core::CandidateId;
use phoinix_device::{DeviceInfo, platform_enumerator};
use phoinix_fs::{DeletedFileProvider, RecoveryCandidate};
use phoinix_health::CandidateSource;

use crate::SessionError;
use crate::dto::{
    DestinationInfo, Preview, RecoverEvent, RecoverItem, RecoverRequest, ScanEvent, ScanRequest,
    SearchEvent, SessionSummary, SourceInfo,
};
use crate::scan::ScanOutcome;
use crate::session::{EXTENSION, ScanSession};
use crate::source::{VolumeChoice, inspect, open_volume_with, search_partitions};
use crate::{preview, recover, scan};
use phoinix_partition_recovery::{PartitionCandidate, SearchOptions};

/// A running scan.
pub struct ScanHandle {
    /// Events, in order.
    pub events: Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<ScanOutcome>>,
}

impl ScanHandle {
    /// Requests cancellation; the scan stops at the next opportunity.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Waits for the scan and returns the (possibly partial) session with
    /// the outcome.
    pub fn wait(mut self) -> ScanOutcome {
        match self.join.take().map(JoinHandle::join) {
            Some(Ok(r)) => r,
            Some(Err(_)) => ScanOutcome {
                session: None,
                result: Err(SessionError::Invalid("the scan thread panicked".into())),
            },
            None => ScanOutcome {
                session: None,
                result: Err(SessionError::Invalid(
                    "the scan was already collected".into(),
                )),
            },
        }
    }
}

/// A running structure search.
pub struct SearchHandle {
    /// Events, in order; the last one is `Finished` or `Failed`.
    pub events: Receiver<SearchEvent>,
    join: Option<JoinHandle<Result<Vec<PartitionCandidate>, SessionError>>>,
}

impl SearchHandle {
    /// Waits for the search.
    pub fn wait(mut self) -> Result<Vec<PartitionCandidate>, SessionError> {
        match self.join.take().map(JoinHandle::join) {
            Some(Ok(r)) => r,
            _ => Err(SessionError::Invalid("the search thread failed".into())),
        }
    }
}

/// The application workspace.
#[derive(Debug)]
pub struct Workspace {
    sessions_dir: PathBuf,
    current: Mutex<Option<Arc<ScanSession>>>,
}

impl Workspace {
    /// A workspace storing sessions under `sessions_dir`.
    #[must_use]
    pub fn new(sessions_dir: impl Into<PathBuf>) -> Self {
        Self {
            sessions_dir: sessions_dir.into(),
            current: Mutex::new(None),
        }
    }

    /// The sessions directory.
    #[must_use]
    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    /// Lists block devices.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Device`] if enumeration fails.
    pub fn devices(&self) -> Result<Vec<DeviceInfo>, SessionError> {
        Ok(platform_enumerator().enumerate()?)
    }

    /// Describes a source.
    ///
    /// # Errors
    ///
    /// See [`inspect`].
    pub fn inspect(&self, path: &Path) -> Result<SourceInfo, SessionError> {
        inspect(path)
    }

    /// Runs the structure search (lost partitions) synchronously.
    ///
    /// # Errors
    ///
    /// See [`search_partitions`].
    pub fn find_partitions(
        &self,
        path: &Path,
        options: &SearchOptions,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<Vec<PartitionCandidate>, SessionError> {
        search_partitions(path, options, progress)
    }

    /// Starts the structure search on a background thread.
    #[must_use]
    pub fn start_partition_search(&self, path: PathBuf, options: SearchOptions) -> SearchHandle {
        let (tx, rx) = channel();
        let join = std::thread::Builder::new()
            .name("phoinix-partition-search".into())
            .spawn(move || {
                let progress_tx = tx.clone();
                let mut progress = |done: u64, total: u64| {
                    let _ = progress_tx.send(SearchEvent::Progress { done, total });
                };
                let result = search_partitions(&path, &options, &mut progress);
                match &result {
                    Ok(candidates) => {
                        let _ = tx.send(SearchEvent::Finished {
                            candidates: candidates.clone(),
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(SearchEvent::Failed {
                            message: e.to_string(),
                        });
                    }
                }
                result
            })
            .ok();
        SearchHandle { events: rx, join }
    }

    /// Starts a scan on a background thread.
    #[must_use]
    pub fn start_scan(&self, request: ScanRequest) -> ScanHandle {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let join = std::thread::Builder::new()
            .name("phoinix-scan".into())
            .spawn(move || {
                let mut sink = |e: ScanEvent| {
                    let _ = tx.send(e);
                };
                scan::run_scan(&request, &mut sink, &flag)
            })
            .ok();
        ScanHandle {
            events: rx,
            cancel,
            join,
        }
    }

    /// Runs a scan synchronously (tests, CLI).
    pub fn scan(&self, request: &ScanRequest, sink: &mut dyn FnMut(ScanEvent)) -> ScanOutcome {
        scan::run_scan(request, sink, &AtomicBool::new(false))
    }

    /// Makes `session` the current one.
    pub fn set_current(&self, session: ScanSession) -> Arc<ScanSession> {
        let arc = Arc::new(session);
        if let Ok(mut guard) = self.current.lock() {
            *guard = Some(arc.clone());
        }
        arc
    }

    /// The current session.
    #[must_use]
    pub fn current(&self) -> Option<Arc<ScanSession>> {
        self.current.lock().ok().and_then(|g| g.clone())
    }

    /// The current session or an error.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Invalid`] when no session is loaded.
    pub fn require_current(&self) -> Result<Arc<ScanSession>, SessionError> {
        self.current()
            .ok_or_else(|| SessionError::Invalid("no scan session is loaded".into()))
    }

    /// Saves `session` into the sessions directory (or to `path`).
    ///
    /// # Errors
    ///
    /// See [`ScanSession::save`].
    pub fn save_session(
        &self,
        session: &mut ScanSession,
        path: Option<&Path>,
    ) -> Result<PathBuf, SessionError> {
        let path = path.map_or_else(
            || self.sessions_dir.join(session.default_file_name()),
            Path::to_path_buf,
        );
        session.save(&path)?;
        Ok(path)
    }

    /// Loads a session file and makes it current.
    ///
    /// # Errors
    ///
    /// See [`ScanSession::load`].
    pub fn load_session(&self, path: &Path) -> Result<Arc<ScanSession>, SessionError> {
        Ok(self.set_current(ScanSession::load(path)?))
    }

    /// Summaries of the session files in the sessions directory, newest
    /// first. Unreadable files are skipped.
    #[must_use]
    pub fn list_sessions(&self) -> Vec<SessionSummary> {
        let Ok(entries) = std::fs::read_dir(&self.sessions_dir) else {
            return Vec::new();
        };
        let mut out: Vec<SessionSummary> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|p| ScanSession::load_summary(&p).ok())
            .collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.started));
        out
    }

    /// A candidate of the current session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::NotFound`].
    pub fn candidate(&self, id: CandidateId) -> Result<RecoveryCandidate, SessionError> {
        let session = self.require_current()?;
        session
            .candidate(id)
            .cloned()
            .ok_or_else(|| SessionError::NotFound(format!("candidate {id} not found")))
    }

    /// Assesses a destination against the current session's source.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::Invalid`] when no session is loaded.
    pub fn destination_info(&self, destination: &Path) -> Result<DestinationInfo, SessionError> {
        let session = self.require_current()?;
        Ok(recover::destination_info(&session.source, destination))
    }

    /// Recovers candidates of the current session.
    ///
    /// # Errors
    ///
    /// See [`recover::recover`].
    pub fn recover(
        &self,
        request: &RecoverRequest,
        sink: &mut dyn FnMut(RecoverEvent),
    ) -> Result<Vec<RecoverItem>, SessionError> {
        let session = self.require_current()?;
        recover::recover(&session, request, sink)
    }

    /// Builds a preview of a candidate of the current session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] if the source cannot be reopened.
    pub fn preview(&self, id: CandidateId) -> Result<Preview, SessionError> {
        let session = self.require_current()?;
        let candidate = session
            .candidate(id)
            .ok_or_else(|| SessionError::NotFound(format!("candidate {id} not found")))?;
        let volume = open_volume_with(
            &session.source,
            &VolumeChoice::reopen(&session.volume),
            false,
        )?;
        let carver;
        let provider: &dyn DeletedFileProvider =
            if candidate.evidence.source == CandidateSource::FileCarving {
                carver = phoinix_carve::CarveEngine::new(
                    volume.reader.clone(),
                    volume.space.clone(),
                    volume.info.filesystem,
                    volume.storage.clone(),
                );
                &carver
            } else {
                volume.engine.as_deref().ok_or_else(|| {
                    SessionError::Invalid(format!("no engine for {}", volume.info.filesystem))
                })?
            };
        Ok(preview::preview(provider, candidate))
    }
}
