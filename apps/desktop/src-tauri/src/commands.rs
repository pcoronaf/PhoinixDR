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
use phoinix_partition_recovery::{PartitionCandidate, SearchOptions};
use phoinix_session::dto::{
    CandidateSummary, DestinationInfo, Preview, RecoverItem, RecoverRequest, ScanRequest,
    SessionSummary, SourceInfo,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{ActiveErase, ActiveScan, AppState};

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
    /// Author attribution.
    pub author: String,
    /// The disclaimer.
    pub disclaimer: String,
    /// Where sessions are stored.
    pub sessions_dir: PathBuf,
    /// Whether the process can enumerate devices at all.
    pub device_access: bool,
    /// Whether the process already runs with administrative rights.
    pub elevated: bool,
    /// The operating system (`windows`, `linux`, `macos`).
    pub platform: &'static str,
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

/// Hashes a source and compares with the hashes its container stores;
/// progress arrives as `verify-event`.
#[tauri::command]
pub async fn verify_source(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<phoinix_image::HashVerification, String> {
    let workspace = state.workspace.clone();
    let emitter = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut last = 0u64;
        workspace
            .verify_source(&PathBuf::from(path), &mut |done, total| {
                if done.saturating_sub(last) >= 32 * 1024 * 1024 || done == total {
                    last = done;
                    let _ = emitter.emit(
                        "verify-event",
                        &serde_json::json!({ "done": done, "total": total }),
                    );
                }
                true
            })
            .map_err(String::from)
    })
    .await
    .map_err(|e| e.to_string())?
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
        author: "@pcoronaf".to_owned(),
        disclaimer: phoinix_core::DISCLAIMER.to_owned(),
        sessions_dir: state.workspace.sessions_dir().to_path_buf(),
        device_access: state.workspace.devices().is_ok(),
        elevated: crate::elevate::is_elevated(),
        platform: std::env::consts::OS,
    }
}

/// Turns the live engine log (`engine-log` events) on or off. Returns the
/// new state.
#[tauri::command]
pub fn set_engine_log(switch: State<'_, crate::enginelog::EngineLogSwitch>, enabled: bool) -> bool {
    switch.set(enabled);
    switch.enabled()
}

/// Starts an elevated copy of PhoinixDR through the operating system's
/// consent prompt (UAC on Windows, polkit on Linux). Returns whether this
/// instance is about to exit: on Windows it closes once the elevated copy
/// has started; on Linux it stays open because `pkexec` cannot report the
/// outcome before the user answers the prompt.
#[tauri::command]
pub fn relaunch_elevated(app: AppHandle) -> Result<bool, String> {
    let exits = crate::elevate::relaunch_elevated()?;
    if exits {
        // Let the reply reach the front-end before this instance goes away.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(400));
            app.exit(0);
        });
    }
    Ok(exits)
}

/// Runs the structure search (lost partitions); progress arrives as
/// `search-event`. Returns the candidates in start order.
#[tauri::command]
pub async fn find_partitions(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    verify: Option<bool>,
) -> Result<Vec<PartitionCandidate>, String> {
    let workspace = state.workspace.clone();
    let options = SearchOptions {
        verify: verify.unwrap_or(true),
        ..Default::default()
    };
    tauri::async_runtime::spawn_blocking(move || {
        let handle = workspace.start_partition_search(PathBuf::from(path), options);
        for event in handle.events.iter() {
            let _ = app.emit("search-event", &event);
        }
        handle.wait().map_err(String::from)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// Secure erase (M12). Events arrive as `erase-event` (`EraseEvent`).

/// One erase method for the method picker.
#[derive(Debug, Clone, Serialize)]
pub struct EraseMethodInfo {
    /// Identifier.
    pub id: String,
    /// Name.
    pub label: String,
    /// Guidance.
    pub note: String,
    /// Number of passes.
    pub passes: usize,
    /// Whether the multi-pass warning applies.
    pub multi_pass: bool,
}

/// Everything the erase pages need to show before a target is chosen.
#[derive(Debug, Clone, Serialize)]
pub struct EraseCatalogue {
    /// Methods in display order.
    pub methods: Vec<EraseMethodInfo>,
    /// The irreversibility warning.
    pub warning: String,
    /// The multi-pass warning.
    pub multi_pass_warning: String,
    /// The solid-state warning.
    pub flash_warning: String,
    /// Article on the Gutmann myth.
    pub article_gutmann: String,
    /// Article on secure deletion of files.
    pub article_files: String,
    /// What a raw free-space wipe cannot reach.
    pub free_space_note: String,
    /// What a mounted (filesystem-level) wipe cannot reach.
    pub mounted_note: String,
    /// What erasing files by name cannot reach.
    pub files_note: String,
    /// What a firmware sanitize is and is not.
    pub firmware_note: String,
    /// What to do about a frozen ATA security feature set.
    pub frozen_advice: String,
    /// Every firmware action with its label and note (the device says
    /// which it supports).
    pub firmware_actions: Vec<EraseMethodInfo>,
}

/// The method catalogue and warning texts.
#[tauri::command]
pub fn erase_catalogue() -> EraseCatalogue {
    EraseCatalogue {
        methods: phoinix_erase::EraseMethod::builtin()
            .iter()
            .map(|m| EraseMethodInfo {
                id: m.id(),
                label: m.label(),
                note: m.note().to_owned(),
                passes: m.pass_count(),
                multi_pass: m.multi_pass(),
            })
            .collect(),
        warning: phoinix_erase::ERASE_WARNING.to_owned(),
        multi_pass_warning: phoinix_erase::MULTI_PASS_WARNING.to_owned(),
        flash_warning: phoinix_erase::FLASH_WARNING.to_owned(),
        article_gutmann: phoinix_erase::ARTICLE_GUTMANN_MYTH.to_owned(),
        article_files: phoinix_erase::ARTICLE_SECURE_DELETION.to_owned(),
        free_space_note: phoinix_erase::FREE_SPACE_NOTE.to_owned(),
        mounted_note: phoinix_erase::MOUNTED_NOTE.to_owned(),
        files_note: phoinix_erase::FILES_NOTE.to_owned(),
        firmware_note: phoinix_erase::FIRMWARE_NOTE.to_owned(),
        frozen_advice: phoinix_erase::FROZEN_ADVICE.to_owned(),
        firmware_actions: phoinix_erase::FirmwareAction::ALL
            .iter()
            .map(|a| EraseMethodInfo {
                id: a.id().to_owned(),
                label: a.label().to_owned(),
                note: a.note().to_owned(),
                passes: 1,
                multi_pass: false,
            })
            .collect(),
    }
}

/// Disks as erase targets, with the refusal rules applied.
#[tauri::command]
pub fn erase_targets(
    state: State<'_, AppState>,
) -> Result<Vec<phoinix_session::EraseTargetInfo>, String> {
    state.workspace.erase_targets().map_err(String::from)
}

/// Mounted volumes as targets of the filesystem-level free-space wipe.
#[tauri::command]
pub fn erase_volumes(state: State<'_, AppState>) -> Vec<phoinix_erase::MountedVolumeInfo> {
    state.workspace.erase_volumes()
}

/// Describes one target (device, partition, file, free space or mounted
/// volume) with the guards applied.
#[tauri::command]
pub fn describe_erase_target(
    state: State<'_, AppState>,
    target: phoinix_erase::EraseTarget,
) -> Result<phoinix_erase::TargetDescription, String> {
    state
        .workspace
        .describe_erase_target(&target)
        .map_err(String::from)
}

/// Starts an erase; events arrive as `erase-event`.
#[tauri::command]
pub fn start_erase(
    app: AppHandle,
    state: State<'_, AppState>,
    request: phoinix_erase::EraseRequest,
) -> Result<(), String> {
    let mut slot = state.erase();
    if slot.is_some() {
        return Err("an erase is already running".into());
    }
    if state.scan_running() {
        return Err("a scan is running; wait for it to finish".into());
    }
    let mut request = request;
    if request.state_dir.is_none() {
        request.state_dir = Some(state.workspace.sessions_dir().to_path_buf());
    }
    let handle = state.workspace.start_erase(request);
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    *slot = Some(ActiveErase {
        cancel: cancel.clone(),
    });
    drop(slot);
    let forwarder = app.clone();
    std::thread::Builder::new()
        .name("phoinix-erase-events".into())
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
                        let _ = forwarder.emit("erase-event", &event);
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            if let Err(e) = handle.wait() {
                // Refusals and open failures happen before any event.
                let _ = forwarder.emit(
                    "erase-event",
                    &phoinix_erase::EraseEvent::Failed {
                        message: e.to_string(),
                        summary: phoinix_erase::EraseSummary::default(),
                    },
                );
            }
            if let Some(state) = forwarder.try_state::<AppState>() {
                *state.erase() = None;
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Requests cancellation of the running erase.
#[tauri::command]
pub fn cancel_erase(state: State<'_, AppState>) -> bool {
    match state.erase().as_ref() {
        Some(active) => {
            active.cancel.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

/// Whether an erase is running.
#[tauri::command]
pub fn erase_running(state: State<'_, AppState>) -> bool {
    state.erase().is_some()
}
