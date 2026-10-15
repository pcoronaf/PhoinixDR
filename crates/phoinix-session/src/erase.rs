//! The erase job for front-ends (milestone M12): target listing with the
//! refusal rules applied, a background erase with events, cancellation.
//! The writing itself lives in `phoinix-erase` (ADR-0014).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;

use phoinix_device::DeviceInfo;
use phoinix_erase::{
    EraseEvent, EraseOutcome, EraseRequest, EraseTarget, Guards, Refusal, TargetDescription,
};
use serde::{Deserialize, Serialize};

use crate::SessionError;

/// A device as an erase target, with the refusal rules already applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraseTargetInfo {
    /// The device.
    pub device: DeviceInfo,
    /// Its description as a whole-device target (partitions, mounted
    /// volumes, refusals), when it could be read.
    pub description: Option<TargetDescription>,
    /// Why the whole device cannot be erased; empty when it can.
    pub refusals: Vec<Refusal>,
    /// Error text when the device could not be described.
    pub error: Option<String>,
}

/// A running erase.
pub struct EraseHandle {
    /// Events, in order.
    pub events: Receiver<EraseEvent>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<Result<EraseOutcome, phoinix_erase::EraseError>>>,
}

impl EraseHandle {
    /// Requests cancellation; the erase stops after the current chunk and
    /// leaves a resume point.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Waits for the erase.
    ///
    /// # Errors
    ///
    /// Returns the erase error (refusal, I/O) or a thread failure.
    pub fn wait(mut self) -> Result<EraseOutcome, SessionError> {
        match self.join.take().map(JoinHandle::join) {
            Some(Ok(r)) => r.map_err(|e| SessionError::Invalid(e.to_string())),
            Some(Err(_)) => Err(SessionError::Invalid("the erase thread panicked".into())),
            None => Err(SessionError::Invalid(
                "the erase was already collected".into(),
            )),
        }
    }
}

/// Lists every enumerated disk as an erase target.
///
/// # Errors
///
/// Returns [`SessionError`] when devices cannot be enumerated at all.
pub fn targets(guards: &Guards) -> Result<Vec<EraseTargetInfo>, SessionError> {
    let devices = phoinix_device::platform_enumerator().enumerate()?;
    Ok(devices
        .into_iter()
        .filter(|d| d.kind == phoinix_device::DeviceKind::Disk)
        .map(|device| {
            let target = EraseTarget::WholeDevice {
                path: device.path.to_path_buf(),
            };
            match phoinix_erase::describe(&target, guards) {
                Ok(description) => EraseTargetInfo {
                    refusals: description.refusals.clone(),
                    description: Some(description),
                    error: None,
                    device,
                },
                Err(e) => EraseTargetInfo {
                    device,
                    description: None,
                    refusals: Vec::new(),
                    error: Some(e.to_string()),
                },
            }
        })
        .collect())
}

/// Describes one target with the guards applied.
///
/// # Errors
///
/// Returns [`SessionError`] when the target cannot be read.
pub fn describe(target: &EraseTarget, guards: &Guards) -> Result<TargetDescription, SessionError> {
    phoinix_erase::describe(target, guards).map_err(|e| SessionError::Invalid(e.to_string()))
}

/// Starts an erase on a background thread.
#[must_use]
pub fn start(request: EraseRequest, guards: Guards) -> EraseHandle {
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    let join = std::thread::Builder::new()
        .name("phoinix-erase".into())
        .spawn(move || {
            let mut sink = |e: EraseEvent| {
                let _ = tx.send(e);
            };
            phoinix_erase::engine::run_erase_with_guards(&request, &guards, &mut sink, &flag)
        })
        .ok();
    EraseHandle {
        events: rx,
        cancel,
        join,
    }
}

/// The guards a workspace applies: its sessions directory and the source
/// of the current session.
#[must_use]
pub fn guards(sessions_dir: &std::path::Path, open_source: Option<PathBuf>) -> Guards {
    Guards {
        sessions_dir: Some(sessions_dir.to_path_buf()),
        open_sources: open_source.into_iter().collect(),
        allow_executable_disk: false,
        allow_sessions_disk: false,
    }
}
