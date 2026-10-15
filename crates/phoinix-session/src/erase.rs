//! The erase job for front-ends (milestone M12): target listing with the
//! refusal rules applied, a background erase with events, cancellation,
//! and the post-erase scan that looks at the target with the recovery
//! engines (M12.2). The writing itself lives in `phoinix-erase`
//! (ADR-0014).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::Instant;

use phoinix_device::DeviceInfo;
use phoinix_erase::{
    EraseEvent, EraseOutcome, EraseRequest, EraseTarget, Guards, MountedVolumeInfo, PostScan,
    PostScanner, Refusal, ScanProgress, ScanRecord, TargetDescription,
};
use phoinix_health::CandidateSource;
use serde::{Deserialize, Serialize};

use crate::SessionError;
use crate::dto::{CarveSettings, ScanEvent, ScanMode, ScanPhase, ScanRequest, VolumeRange};

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

/// Mounted volumes as targets of the filesystem-level free-space wipe.
#[must_use]
pub fn volumes() -> Vec<MountedVolumeInfo> {
    phoinix_erase::mounted_volumes()
}

/// Starts an erase on a background thread, with the post-erase scanner.
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
            phoinix_erase::run_erase_with(
                &request,
                &guards,
                Some(&SessionScanner),
                &mut sink,
                &flag,
            )
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
pub fn guards(sessions_dir: &Path, open_source: Option<PathBuf>) -> Guards {
    Guards {
        sessions_dir: Some(sessions_dir.to_path_buf()),
        open_sources: open_source.into_iter().collect(),
        allow_executable_disk: false,
        allow_sessions_disk: false,
    }
}

/// The post-erase scanner: runs the quick or deep scan of this crate over
/// the erased target and counts what a recovery would still return.
#[derive(Debug, Clone, Copy, Default)]
pub struct SessionScanner;

/// Whether a candidate's examined content was something other than zeros.
/// A deleted record whose clusters all belong to live files now points at
/// those files' data, not at its own, so it does not count.
fn has_content(c: &phoinix_fs::RecoveryCandidate) -> bool {
    let content = &c.evidence.content;
    if c.evidence.source == CandidateSource::FileCarving {
        return true;
    }
    if c.logical_size.unwrap_or(0) == 0 || content.bytes_examined == 0 {
        return false;
    }
    let alloc = &c.evidence.allocation;
    if alloc.map_available
        && alloc.clusters_total > 0
        && alloc.clusters_allocated == alloc.clusters_total
        && !c.evidence.extents.resident
    {
        return false;
    }
    match content.zero_block_ratio {
        Some(ratio) => ratio < 1.0 || !content.head_is_zero,
        None => !content.head_is_zero,
    }
}

impl SessionScanner {
    /// The scan request for a target, or why none can be made.
    fn request_for(target: &TargetDescription, mode: PostScan) -> Result<ScanRequest, String> {
        let scan_mode = if mode == PostScan::Deep {
            ScanMode::Deep
        } else {
            ScanMode::Quick
        };
        let (source, partition, volume, whole_volume) = match &target.target {
            EraseTarget::WholeDevice { path } | EraseTarget::File { path } => {
                (path.clone(), None, None, true)
            }
            EraseTarget::Partition { path, index } => (path.clone(), Some(*index), None, true),
            EraseTarget::Range {
                path,
                offset,
                length,
            } => (
                path.clone(),
                None,
                Some(VolumeRange {
                    offset: *offset,
                    length: *length,
                    repairs: Vec::new(),
                }),
                true,
            ),
            EraseTarget::FreeSpace { path, .. } => (
                path.clone(),
                target.partition.as_ref().map(|p| p.index),
                None,
                false,
            ),
            EraseTarget::MountedFreeSpace { .. } => {
                let node = target
                    .space
                    .as_ref()
                    .and_then(|s| s.node.clone())
                    .ok_or_else(|| "the volume node is unknown".to_owned())?;
                (PathBuf::from(node), None, None, false)
            }
        };
        Ok(ScanRequest {
            source,
            partition,
            volume,
            mode: scan_mode,
            examine_content: true,
            carve: CarveSettings {
                whole_volume,
                ..Default::default()
            },
        })
    }
}

impl PostScanner for SessionScanner {
    fn scan(
        &self,
        target: &TargetDescription,
        mode: PostScan,
        progress: &mut dyn FnMut(ScanProgress),
        cancel: &AtomicBool,
    ) -> ScanRecord {
        let started = Instant::now();
        let request = match Self::request_for(target, mode) {
            Ok(r) => r,
            Err(why) => return ScanRecord::not_run(mode, why),
        };
        let mut record = ScanRecord {
            mode,
            ran: true,
            ..Default::default()
        };
        // What the target looks like now: partition table and filesystems.
        if let Ok(info) = crate::source::inspect(&request.source) {
            record.partition_table = Some(info.scheme.clone());
            let chosen = match request.partition {
                Some(i) => info.volumes.iter().find(|v| v.partition == Some(i)),
                None => info
                    .volumes
                    .iter()
                    .find(|v| v.supported)
                    .or_else(|| info.volumes.first()),
            };
            record.filesystem = chosen.map(|v| v.filesystem.to_string());
        }
        let mut candidates = 0u64;
        let mut sink = |e: ScanEvent| match e {
            ScanEvent::Phase { phase } => progress(ScanProgress {
                phase: phase_name(phase).to_owned(),
                done: 0,
                total: None,
                candidates,
            }),
            ScanEvent::Progress {
                phase,
                done,
                total,
                candidates: n,
                ..
            } => {
                candidates = n;
                progress(ScanProgress {
                    phase: phase_name(phase).to_owned(),
                    done,
                    total,
                    candidates: n,
                });
            }
            _ => {}
        };
        let outcome = crate::scan::run_scan(&request, &mut sink, cancel);
        record.seconds = started.elapsed().as_secs();
        match &outcome.result {
            Ok(()) => {}
            Err(SessionError::Cancelled) => {
                record.cancelled = true;
                record.note = Some("cancelled".into());
            }
            Err(e) => {
                let no_filesystem = record
                    .filesystem
                    .as_deref()
                    .is_none_or(|f| f.eq_ignore_ascii_case("unknown"));
                let nothing_found = outcome
                    .session
                    .as_ref()
                    .is_none_or(|s| s.candidates.is_empty());
                if no_filesystem && nothing_found {
                    // No filesystem to walk: after a whole-target overwrite
                    // that is the expected outcome.
                    record.note = Some(format!("no filesystem to walk ({e})"));
                    record.passed = true;
                } else {
                    record.ran = false;
                    record.note = Some(e.to_string());
                }
            }
        }
        if let Some(session) = &outcome.session {
            let mut resident_with_content = 0u64;
            for c in &session.candidates {
                if c.evidence.source == CandidateSource::FileCarving {
                    record.carved += 1;
                    continue;
                }
                record.candidates += 1;
                if has_content(c) {
                    if c.evidence.extents.resident {
                        resident_with_content += 1;
                    } else {
                        record.with_content += 1;
                    }
                }
            }
            if let Some(report) = &session.carving {
                record.bytes_scanned = report.bytes_scanned;
            }
            if resident_with_content > 0 {
                record.note = Some(format!(
                    "{resident_with_content} deleted record(s) keep their content inside the filesystem metadata (resident data), which a free-space wipe cannot reach"
                ));
            }
            if outcome.result.is_ok() {
                record.passed = record.with_content == 0 && record.carved == 0;
            }
        }
        record
    }
}

const fn phase_name(phase: ScanPhase) -> &'static str {
    match phase {
        ScanPhase::Opening => "opening",
        ScanPhase::Metadata => "metadata",
        ScanPhase::Carving => "carving",
        ScanPhase::Assembling => "assembling",
        ScanPhase::Finishing => "finishing",
    }
}
