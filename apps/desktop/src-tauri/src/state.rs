//! Application state shared by the commands.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use phoinix_session::Workspace;

/// A scan in progress.
pub struct ActiveScan {
    /// Cancellation flag shared with the scan thread.
    pub cancel: Arc<AtomicBool>,
}

/// State managed by Tauri.
pub struct AppState {
    /// The workspace (sessions, current session).
    pub workspace: Arc<Workspace>,
    scan: Mutex<Option<ActiveScan>>,
    recovering: AtomicBool,
}

impl AppState {
    /// A state storing sessions under `sessions_dir`.
    #[must_use]
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self {
            workspace: Arc::new(Workspace::new(sessions_dir)),
            scan: Mutex::new(None),
            recovering: AtomicBool::new(false),
        }
    }

    /// The active scan slot.
    pub fn scan(&self) -> MutexGuard<'_, Option<ActiveScan>> {
        match self.scan.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Whether a scan is running.
    #[must_use]
    pub fn scan_running(&self) -> bool {
        self.scan().is_some()
    }

    /// Marks recovery as running; returns `false` if one already is.
    pub fn begin_recovery(&self) -> bool {
        self.recovering
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Marks recovery as finished.
    pub fn end_recovery(&self) {
        self.recovering.store(false, Ordering::Release);
    }
}
