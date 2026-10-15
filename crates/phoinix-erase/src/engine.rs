//! The erase job: passes × chunks, progress, cancellation, resume,
//! verification and the certificate.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use phoinix_core::ByteRange;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::certificate::{Certificate, CertificateStatus, Operator, VerificationRecord};
use crate::error::{EraseError, Refusal};
use crate::method::{EraseMethod, VerifyMode};
use crate::pattern::{CHUNK_BYTES, Seed, fill_chunk};
use crate::scan::{PostScan, PostScanner, ScanRecord};
use crate::target::{EraseTarget, Guards, TargetDescription, describe};
use crate::verify::{SpanMap, read_back};
use crate::writer::write_aligned;

/// A request to erase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraseRequest {
    /// What to erase.
    pub target: EraseTarget,
    /// How.
    pub method: EraseMethod,
    /// Read-back verification.
    #[serde(default)]
    pub verify: VerifyMode,
    /// Who.
    #[serde(default)]
    pub operator: Operator,
    /// Where to write the certificate (`.json`, `.md`, `.html`).
    #[serde(default)]
    pub certificate: Option<PathBuf>,
    /// The confirmation text the user typed (serial number or path name).
    pub confirmation: String,
    /// Continue a cancelled erase of the same target and method from its
    /// resume point instead of starting over.
    #[serde(default)]
    pub resume: bool,
    /// Directory for resume points (none: no resume support).
    #[serde(default)]
    pub state_dir: Option<PathBuf>,
    /// Allow the disk holding the executable.
    #[serde(default)]
    pub allow_executable_disk: bool,
    /// Allow the disk holding the sessions directory.
    #[serde(default)]
    pub allow_sessions_disk: bool,
    /// Scan the target with the recovery engines afterwards and record the
    /// outcome in the certificate (needs a front-end that provides the
    /// scanner: the session layer does).
    #[serde(default)]
    pub post_scan: PostScan,
}

/// Where a cancelled erase can continue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePoint {
    /// Target key (hash of path, offset and length).
    pub key: String,
    /// Method id.
    pub method: String,
    /// Pass index to continue in.
    pub pass: usize,
    /// Byte offset (relative to the target) to continue at.
    pub offset: u64,
    /// The run's seed.
    pub seed: String,
    /// Bytes written so far.
    pub bytes_written: u64,
    /// Unwritable ranges so far.
    pub unwritable: Vec<ByteRange>,
}

/// Progress and outcome events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EraseEvent {
    /// Writing is about to start.
    Started {
        /// The target.
        description: Box<TargetDescription>,
        /// Method label.
        method: String,
        /// Number of passes.
        passes: usize,
        /// Resume point used, if any.
        resumed_from: Option<ResumePoint>,
    },
    /// A pass starts.
    Pass {
        /// 1-based pass index.
        pass: usize,
        /// Number of passes.
        passes: usize,
        /// Pattern label.
        pattern: String,
    },
    /// Progress inside a pass.
    Progress {
        /// 1-based pass index.
        pass: usize,
        /// Number of passes.
        passes: usize,
        /// Bytes written in this pass.
        done: u64,
        /// Bytes of the target.
        total: u64,
        /// Current throughput.
        bytes_per_sec: u64,
        /// Bytes the device refused so far.
        unwritable: u64,
    },
    /// Read-back verification progress.
    Verifying {
        /// Blocks checked.
        done: u64,
        /// Blocks to check.
        total: u64,
    },
    /// Post-erase scan progress.
    Scanning {
        /// The scan.
        mode: PostScan,
        /// Phase name.
        phase: String,
        /// Units done.
        done: u64,
        /// Units in total, if known.
        total: Option<u64>,
        /// Candidates so far.
        candidates: u64,
    },
    /// The erase completed.
    Finished {
        /// Summary.
        summary: EraseSummary,
    },
    /// Cancelled by the user.
    Cancelled {
        /// Where it can resume (none for a mounted volume: its fill files
        /// are removed on cancellation).
        resume: Option<ResumePoint>,
        /// Summary so far.
        summary: EraseSummary,
    },
    /// Failed.
    Failed {
        /// Message.
        message: String,
        /// Summary so far.
        summary: EraseSummary,
    },
}

/// Outcome figures.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EraseSummary {
    /// Passes completed.
    pub passes_completed: usize,
    /// Passes in the method.
    pub passes: usize,
    /// Bytes written in total.
    pub bytes_written: u64,
    /// Bytes the device refused.
    pub unwritable_bytes: u64,
    /// Number of unwritable regions.
    pub unwritable_ranges: usize,
    /// Seconds spent.
    pub seconds: u64,
    /// The verification, if run.
    pub verification: Option<VerificationRecord>,
    /// The post-erase scan, if run.
    #[serde(default)]
    pub scan: Option<ScanRecord>,
    /// Certificate written, if requested.
    pub certificate: Option<PathBuf>,
    /// Verdict text.
    pub verdict: String,
}

/// What [`run_erase`] returns.
#[derive(Debug)]
pub struct EraseOutcome {
    /// The certificate (also written to disk when requested).
    pub certificate: Certificate,
    /// Figures.
    pub summary: EraseSummary,
    /// `Ok` when every pass completed; [`EraseError::Cancelled`] or the
    /// failure otherwise.
    pub result: Result<(), EraseError>,
}

fn target_key(desc: &TargetDescription) -> String {
    let mut h = Sha256::new();
    h.update(desc.path.as_bytes());
    h.update(desc.offset.to_le_bytes());
    h.update(desc.length.to_le_bytes());
    if let Some(s) = desc.device.as_ref().and_then(|d| d.serial.as_deref()) {
        h.update(s.as_bytes());
    }
    let digest = h.finalize();
    hex::encode(digest.get(..16).unwrap_or(&digest))
}

fn resume_path(state_dir: &std::path::Path, key: &str) -> PathBuf {
    state_dir.join(format!("erase-{key}.resume.json"))
}

/// Reads the resume point stored for a target, if any.
#[must_use]
pub fn stored_resume(state_dir: &std::path::Path, desc: &TargetDescription) -> Option<ResumePoint> {
    let text = std::fs::read_to_string(resume_path(state_dir, &target_key(desc))).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_resume(state_dir: Option<&PathBuf>, point: &ResumePoint) {
    if let Some(dir) = state_dir {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(text) = serde_json::to_string_pretty(point) {
            let _ = std::fs::write(resume_path(dir, &point.key), text);
        }
    }
}

fn clear_resume(state_dir: Option<&PathBuf>, key: &str) {
    if let Some(dir) = state_dir {
        let _ = std::fs::remove_file(resume_path(dir, key));
    }
}

/// Runs an erase, sending events to `sink`; `cancel` stops it after the
/// current chunk. The target is described and checked first; nothing is
/// written if it is refused or the confirmation does not match.
///
/// # Errors
///
/// Returns [`EraseError`] for a refusal, a bad confirmation or a target
/// that cannot be opened. Failures after writing started are reported in
/// the outcome's `result` and as [`EraseEvent::Failed`].
pub fn run_erase(
    request: &EraseRequest,
    sink: &mut dyn FnMut(EraseEvent),
    cancel: &AtomicBool,
) -> Result<EraseOutcome, EraseError> {
    let guards = Guards {
        sessions_dir: request.state_dir.clone(),
        open_sources: Vec::new(),
        allow_executable_disk: request.allow_executable_disk,
        allow_sessions_disk: request.allow_sessions_disk,
    };
    run_erase_with(request, &guards, None, sink, cancel)
}

/// [`run_erase`] with explicit guards (open sources, sessions directory).
///
/// # Errors
///
/// See [`run_erase`].
pub fn run_erase_with_guards(
    request: &EraseRequest,
    guards: &Guards,
    sink: &mut dyn FnMut(EraseEvent),
    cancel: &AtomicBool,
) -> Result<EraseOutcome, EraseError> {
    run_erase_with(request, guards, None, sink, cancel)
}

/// [`run_erase`] with explicit guards and the scanner that runs the
/// post-erase scan the request asks for.
///
/// # Errors
///
/// See [`run_erase`].
#[allow(clippy::too_many_lines)]
pub fn run_erase_with(
    request: &EraseRequest,
    guards: &Guards,
    scanner: Option<&dyn PostScanner>,
    sink: &mut dyn FnMut(EraseEvent),
    cancel: &AtomicBool,
) -> Result<EraseOutcome, EraseError> {
    let desc = describe(&request.target, guards)?;
    if let Some(r) = desc.refusals.first() {
        return Err(EraseError::Refused(r.clone()));
    }
    if !desc.confirms(&request.confirmation) {
        return Err(EraseError::Refused(Refusal::NotConfirmed {
            expected: desc
                .confirmation_short
                .clone()
                .unwrap_or_else(|| desc.confirmation.clone()),
        }));
    }
    let passes = request.method.passes();
    if passes.is_empty() {
        return Err(EraseError::Invalid("the method has no passes".into()));
    }
    let key = target_key(&desc);
    if desc.length == 0 && !desc.target.is_mounted() {
        return Err(EraseError::Invalid(
            "nothing to erase: the target is empty".into(),
        ));
    }
    let resumed = request
        .resume
        .then(|| {
            request
                .state_dir
                .as_deref()
                .and_then(|d| stored_resume(d, &desc))
        })
        .flatten()
        .filter(|p| p.method == request.method.id() && p.pass < passes.len());
    let seed = resumed
        .as_ref()
        .and_then(|p| Seed::from_hex(&p.seed))
        .unwrap_or_else(Seed::fresh);
    let mut certificate = Certificate::start(
        &desc,
        &request.method,
        &seed.to_hex(),
        request.operator.clone(),
    );
    let mut unwritable: Vec<ByteRange> = resumed
        .as_ref()
        .map(|p| p.unwritable.clone())
        .unwrap_or_default();
    let mut bytes_written: u64 = resumed.as_ref().map_or(0, |p| p.bytes_written);
    let (start_pass, start_offset) = resumed.as_ref().map_or((0, 0), |p| (p.pass, p.offset));
    if let Some(path) = &request.certificate {
        certificate
            .write_to(path)
            .map_err(|e| EraseError::io(path, e))?;
    }
    tracing::info!(
        target = %desc.path,
        kind = desc.target.kind(),
        offset = desc.offset,
        length = desc.length,
        method = %request.method.id(),
        passes = passes.len(),
        resumed = resumed.is_some(),
        "erase starting"
    );
    sink(EraseEvent::Started {
        description: Box::new(desc.clone()),
        method: request.method.label(),
        passes: passes.len(),
        resumed_from: resumed.clone(),
    });

    let started = Instant::now();
    let mut passes_completed = start_pass;
    let mut outcome: Result<(), EraseError> = Ok(());
    let mut resume_point: Option<ResumePoint> = None;
    let mut verification: Option<VerificationRecord> = None;
    let mounted_reserve = match &desc.target {
        EraseTarget::MountedFreeSpace { reserve, .. } => Some(*reserve),
        _ => None,
    };
    let summary_of = |passes_completed: usize,
                      bytes_written: u64,
                      unwritable: &Vec<ByteRange>,
                      verification: Option<VerificationRecord>,
                      scan: Option<ScanRecord>,
                      certificate: &Certificate| EraseSummary {
        passes_completed,
        passes: passes.len(),
        bytes_written,
        unwritable_bytes: unwritable.iter().map(|r| r.length).sum(),
        unwritable_ranges: unwritable.len(),
        seconds: started.elapsed().as_secs(),
        verification,
        scan,
        certificate: request.certificate.clone(),
        verdict: certificate.verdict(),
    };

    if let Some(reserve) = mounted_reserve {
        // Filesystem-level fill: its own loop, no resume, own read-back.
        let fill =
            crate::mounted::run(&desc, &passes, &seed, reserve, request.verify, sink, cancel);
        bytes_written = fill.bytes_written;
        passes_completed = fill.passes_completed;
        verification = fill.verification;
        outcome = fill.result;
        certificate.target.length = fill.covered;
        if let Some(sp) = certificate.target.space.as_mut() {
            sp.free_bytes = fill.covered;
            sp.free_ranges = fill.files;
        }
    } else {
        let spans = SpanMap::new(&desc.spans());
        let total = spans.total();
        let sector = u64::from(desc.sector_size.max(1));
        let mut writer = crate::platform::open_writer(&desc)?;
        let mut buf = vec![0u8; CHUNK_BYTES];
        let mut filled: Option<(usize, u64)> = None;
        'passes: for (pass_index, pass) in passes.iter().enumerate().skip(start_pass) {
            let label = pass.pattern.label();
            sink(EraseEvent::Pass {
                pass: pass_index + 1,
                passes: passes.len(),
                pattern: label.clone(),
            });
            tracing::info!(pass = pass_index + 1, pattern = %label, "pass starting");
            let mut rel: u64 = if pass_index == start_pass {
                start_offset
            } else {
                0
            };
            let mut last_tick = Instant::now();
            let mut tick_bytes: u64 = 0;
            let mut last_save = Instant::now();
            let mut rate: u64 = 0;
            let point_at = |pass: usize,
                            offset: u64,
                            bytes_written: u64,
                            unwritable: &[ByteRange]| ResumePoint {
                key: key.clone(),
                method: request.method.id(),
                pass,
                offset,
                seed: seed.to_hex(),
                bytes_written,
                unwritable: unwritable.to_vec(),
            };
            while rel < total {
                if cancel.load(Ordering::Relaxed) {
                    let point = point_at(pass_index, rel, bytes_written, &unwritable);
                    save_resume(request.state_dir.as_ref(), &point);
                    resume_point = Some(point);
                    outcome = Err(EraseError::Cancelled);
                    break 'passes;
                }
                let (abs, span_left) = spans.locate(rel);
                let chunk = rel / CHUNK_BYTES as u64;
                let within = usize::try_from(rel % CHUNK_BYTES as u64).unwrap_or(0);
                if filled != Some((pass_index, chunk)) {
                    fill_chunk(&pass.pattern, &seed, pass_index, chunk, &mut buf);
                    filled = Some((pass_index, chunk));
                }
                let want = usize::try_from((CHUNK_BYTES - within) as u64)
                    .unwrap_or(CHUNK_BYTES)
                    .min(usize::try_from(span_left).unwrap_or(CHUNK_BYTES))
                    .min(usize::try_from(total - rel).unwrap_or(CHUNK_BYTES));
                let piece = buf.get(within..within + want).unwrap_or(&[]);
                if let Err(e) = write_aligned(&mut *writer, abs, piece, sector, &mut unwritable) {
                    outcome = Err(EraseError::io(desc.target.path(), e));
                    let point = point_at(pass_index, rel, bytes_written, &unwritable);
                    save_resume(request.state_dir.as_ref(), &point);
                    resume_point = Some(point);
                    break 'passes;
                }
                rel += want as u64;
                bytes_written += want as u64;
                tick_bytes += want as u64;
                // Progress every 250 ms, every 16 MiB, and at the end.
                if last_tick.elapsed() >= Duration::from_millis(250)
                    || rel >= total
                    || tick_bytes >= 16 * 1024 * 1024
                {
                    let millis = u64::try_from(last_tick.elapsed().as_millis())
                        .unwrap_or(u64::MAX)
                        .max(1);
                    rate = tick_bytes.saturating_mul(1000) / millis;
                    sink(EraseEvent::Progress {
                        pass: pass_index + 1,
                        passes: passes.len(),
                        done: rel,
                        total,
                        bytes_per_sec: rate,
                        unwritable: unwritable.iter().map(|r| r.length).sum(),
                    });
                    last_tick = Instant::now();
                    tick_bytes = 0;
                }
                if last_save.elapsed() >= Duration::from_secs(5) {
                    save_resume(
                        request.state_dir.as_ref(),
                        &point_at(pass_index, rel, bytes_written, &unwritable),
                    );
                    last_save = Instant::now();
                }
            }
            if let Err(e) = writer.flush() {
                outcome = Err(EraseError::io(desc.target.path(), e));
                break;
            }
            passes_completed = pass_index + 1;
            tracing::info!(pass = pass_index + 1, bytes_per_sec = rate, "pass finished");
        }
        drop(writer);
    }

    certificate.passes_completed = passes_completed;
    certificate.bytes_written = bytes_written;
    certificate.unwritable = unwritable.clone();

    if outcome.is_ok() && mounted_reserve.is_none() {
        let mode = if request.method.implies_verification() && request.verify == VerifyMode::None {
            VerifyMode::Sample
        } else {
            request.verify
        };
        if let Some(last) = passes.last() {
            let mut progress = |done, total| sink(EraseEvent::Verifying { done, total });
            let plan = crate::verify::ReadBack {
                desc: &desc,
                last: &last.pattern,
                seed: &seed,
                pass_index: passes.len() - 1,
                mode,
                failed: &unwritable,
            };
            match read_back(&plan, &mut progress, cancel) {
                Ok(record) => verification = Some(record),
                Err(e) => {
                    tracing::warn!(error = %e, "read-back verification could not run");
                    verification = Some(VerificationRecord {
                        mode,
                        ..Default::default()
                    });
                }
            }
        }
    }
    if outcome.is_ok() {
        clear_resume(request.state_dir.as_ref(), &key);
    }
    let mut scan: Option<ScanRecord> = None;
    if outcome.is_ok() && request.post_scan != PostScan::None {
        scan = Some(match scanner {
            Some(s) => {
                tracing::info!(mode = request.post_scan.id(), "post-erase scan starting");
                let mut progress = |p: crate::scan::ScanProgress| {
                    sink(EraseEvent::Scanning {
                        mode: request.post_scan,
                        phase: p.phase,
                        done: p.done,
                        total: p.total,
                        candidates: p.candidates,
                    });
                };
                s.scan(&desc, request.post_scan, &mut progress, cancel)
            }
            None => ScanRecord::not_run(
                request.post_scan,
                "this front-end provides no recovery scanner",
            ),
        });
        if let Some(r) = &scan {
            tracing::info!(summary = %r.summary(), "post-erase scan finished");
        }
    }
    certificate.scan = scan.clone();
    certificate.verification = verification.clone();
    certificate.finish(match &outcome {
        Ok(()) => CertificateStatus::Completed,
        Err(EraseError::Cancelled) => CertificateStatus::Cancelled,
        Err(_) => CertificateStatus::Failed,
    });
    if let Some(path) = &request.certificate
        && let Err(e) = certificate.write_to(path)
    {
        tracing::warn!(path = %path.display(), error = %e, "certificate not written");
    }
    let summary = summary_of(
        passes_completed,
        bytes_written,
        &unwritable,
        verification,
        scan,
        &certificate,
    );
    tracing::info!(verdict = %summary.verdict, seconds = summary.seconds, "erase finished");
    match (&outcome, resume_point) {
        (Ok(()), _) => sink(EraseEvent::Finished {
            summary: summary.clone(),
        }),
        (Err(EraseError::Cancelled), resume) => sink(EraseEvent::Cancelled {
            resume,
            summary: summary.clone(),
        }),
        (Err(e), _) => sink(EraseEvent::Failed {
            message: e.to_string(),
            summary: summary.clone(),
        }),
    }
    Ok(EraseOutcome {
        certificate,
        summary,
        result: outcome,
    })
}
