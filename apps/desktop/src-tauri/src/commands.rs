//! Tauri commands: thin, typed wrappers over the service layer. Long
//! operations run on their own threads and report through events:
//! `scan-event` (`ScanEvent`), `scan-complete` (`ScanCompletion`) and
//! `recover-event` (`RecoverEvent`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use phoinix_core::CandidateId;
use phoinix_device::DeviceInfo;
use phoinix_fs::RecoveryCandidate;
use phoinix_session::dto::{
    CandidateSummary, DestinationInfo, Preview, RecoverItem, RecoverRequest, ScanRequest,
    SessionSummary, SourceInfo,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{ActiveScan, AppState};

/// What `scan-complete` carries.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanCompletion {
    /// The scan finished (or was cancelled with partial results).
    Session {
        /// The session, now current and saved.
        summary: Box<SessionSummary>,
        /// Whether it was cancelled.
        cancelled: bool,
    },
    /// The scan failed before producing a session.
    Failed {
        /// Error text.
        message: String,
    },
}

/// Static information about the application.
#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    /// Application version.
    pub version: String,
    /// Where sessions are stored.
    pub sessions_dir: PathBuf,
    /// Whether the process can enumerate devices at all.
    pub device_access: bool,
}

/// Lists block devices.
#[tauri::command]
pub fn list_devices(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    state.workspace.devices().map_err(String::from)
}

/// Describes a source (partition table, volumes, filesystems).
#[tauri::command]
pub fn inspect_source(path: String) -> Result<SourceInfo, String> {
    phoinix_session::source::inspect(&PathBuf::from(path)).map_err(String::from)
}

/// Starts a scan; events arrive as `scan-event`, then `scan-complete`.
#[tauri::command]
pub fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ScanRequest,
) -> Result<(), String> {
    let mut slot = state.scan();
    if slot.is_some() {
        return Err("a scan is already running".into());
    }
    let workspace = state.workspace.clone();
    let handle = workspace.start_scan(request);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    *slot = Some(ActiveScan {
        cancel: cancel.clone(),
    });
    drop(slot);
    let forwarder = app.clone();
    std::thread::Builder::new()
        .name("phoinix-scan-events".into())
        .spawn(move || {
            let mut cancelled_by_user = false;
            loop {
                if cancel.load(Ordering::Relaxed) && !cancelled_by_user {
                    handle.cancel();
                    cancelled_by_user = true;
                }
                match handle
                    .events
                    .recv_timeout(std::time::Duration::from_millis(200))
                {
                    Ok(event) => {
                        let _ = forwarder.emit("scan-event", &event);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            let outcome = handle.wait();
            let completion = match outcome.session {
                Some(mut session) => {
                    if let Err(e) = workspace.save_session(&mut session, None) {
                        tracing::warn!(error = %e, "session not saved");
                    }
                    let summary = Box::new(workspace.set_current(session).summary());
                    ScanCompletion::Session {
                        summary,
                        cancelled: matches!(
                            outcome.result,
                            Err(phoinix_session::SessionError::Cancelled)
                        ),
                    }
                }
                None => ScanCompletion::Failed {
                    message: outcome
                        .result
                        .err()
                        .map_or_else(|| "unknown failure".to_owned(), |e| e.to_string()),
                },
            };
            if let Some(state) = forwarder.try_state::<AppState>() {
                *state.scan() = None;
            }
            let _ = forwarder.emit("scan-complete", &completion);
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Cancels the running scan, if any.
#[tauri::command]
pub fn cancel_scan(state: State<'_, AppState>) -> bool {
    match state.scan().as_ref() {
        Some(active) => {
            active.cancel.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Whether a scan is running.
#[tauri::command]
pub fn scan_running(state: State<'_, AppState>) -> bool {
    state.scan_running()
}

/// Stored sessions, newest first.
#[tauri::command]
pub fn list_sessions(state: State<'_, AppState>) -> Vec<SessionSummary> {
    state.workspace.list_sessions()
}

/// Loads a session file and makes it current.
#[tauri::command]
pub fn load_session(state: State<'_, AppState>, path: String) -> Result<SessionSummary, String> {
    state
        .workspace
        .load_session(&PathBuf::from(path))
        .map(|s| s.summary())
        .map_err(String::from)
}

/// The current session's summary.
#[tauri::command]
pub fn current_session(state: State<'_, AppState>) -> Option<SessionSummary> {
    state.workspace.current().map(|s| s.summary())
}

/// Rows of the current session.
#[tauri::command]
pub fn candidates(state: State<'_, AppState>) -> Result<Vec<CandidateSummary>, String> {
    Ok(state
        .workspace
        .require_current()
        .map_err(String::from)?
        .summaries())
}

/// Full evidence of one candidate.
#[tauri::command]
pub fn candidate_detail(
    state: State<'_, AppState>,
    id: CandidateId,
) -> Result<RecoveryCandidate, String> {
    state.workspace.candidate(id).map_err(String::from)
}

/// A preview of one candidate (runs off the main thread).
#[tauri::command]
pub async fn preview_candidate(
    state: State<'_, AppState>,
    id: CandidateId,
) -> Result<Preview, String> {
    let workspace = state.workspace.clone();
    tauri::async_runtime::spawn_blocking(move || workspace.preview(id).map_err(String::from))
        .await
        .map_err(|e| e.to_string())?
}

/// Assesses a destination against the current session's source.
#[tauri::command]
pub fn check_destination(
    state: State<'_, AppState>,
    destination: String,
) -> Result<DestinationInfo, String> {
    state
        .workspace
        .destination_info(&PathBuf::from(destination))
        .map_err(String::from)
}

/// Recovers candidates; progress arrives as `recover-event`.
#[tauri::command]
pub async fn recover(
    app: AppHandle,
    state: State<'_, AppState>,
    request: RecoverRequest,
) -> Result<Vec<RecoverItem>, String> {
    if !state.begin_recovery() {
        return Err("a recovery is already running".into());
    }
    let workspace = state.workspace.clone();
    let emitter = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut sink = |e| {
            let _ = emitter.emit("recover-event", &e);
        };
        workspace.recover(&request, &mut sink).map_err(String::from)
    })
    .await
    .map_err(|e| e.to_string());
    state.end_recovery();
    result?
}

/// Static application information.
#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        sessions_dir: state.workspace.sessions_dir().to_path_buf(),
        device_access: state.workspace.devices().is_ok(),
    }
}
