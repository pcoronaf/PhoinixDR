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
use crate::target::{EraseTarget, Guards, TargetDescription, describe};
use crate::verify::read_back;
use crate::writer::write_tolerant;

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
    /// The erase completed.
    Finished {
        /// Summary.
        summary: EraseSummary,
    },
    /// Cancelled by the user.
    Cancelled {
        /// Where it can resume.
        resume: ResumePoint,
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
    run_erase_with_guards(request, &guards, sink, cancel)
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
    let mut writer = crate::platform::open_writer(&desc)?;
    let mut buf = vec![0u8; CHUNK_BYTES];
    let mut passes_completed = start_pass;
    let mut outcome: Result<(), EraseError> = Ok(());
    let mut resume_point: Option<ResumePoint> = None;
    let summary_of = |passes_completed: usize,
                      bytes_written: u64,
                      unwritable: &Vec<ByteRange>,
                      verification: Option<VerificationRecord>,
                      certificate: &Certificate| EraseSummary {
        passes_completed,
        passes: passes.len(),
        bytes_written,
        unwritable_bytes: unwritable.iter().map(|r| r.length).sum(),
        unwritable_ranges: unwritable.len(),
        seconds: started.elapsed().as_secs(),
        verification,
        certificate: request.certificate.clone(),
        verdict: certificate.verdict(),
    };

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
        while rel < desc.length {
            if cancel.load(Ordering::Relaxed) {
                let point = ResumePoint {
                    key: key.clone(),
                    method: request.method.id(),
                    pass: pass_index,
                    offset: rel,
                    seed: seed.to_hex(),
                    bytes_written,
                    unwritable: unwritable.clone(),
                };
                save_resume(request.state_dir.as_ref(), &point);
                resume_point = Some(point);
                outcome = Err(EraseError::Cancelled);
                break 'passes;
            }
            let chunk = rel / CHUNK_BYTES as u64;
            let want =
                usize::try_from((desc.length - rel).min(CHUNK_BYTES as u64)).unwrap_or(CHUNK_BYTES);
            let slot = buf.get_mut(..want).unwrap_or(&mut []);
            fill_chunk(&pass.pattern, &seed, pass_index, chunk, slot);
            if let Err(e) = write_tolerant(&mut *writer, desc.offset + rel, slot, &mut unwritable) {
                outcome = Err(EraseError::io(desc.target.path(), e));
                let point = ResumePoint {
                    key: key.clone(),
                    method: request.method.id(),
                    pass: pass_index,
                    offset: rel,
                    seed: seed.to_hex(),
                    bytes_written,
                    unwritable: unwritable.clone(),
                };
                save_resume(request.state_dir.as_ref(), &point);
                resume_point = Some(point);
                break 'passes;
            }
            rel += want as u64;
            bytes_written += want as u64;
            tick_bytes += want as u64;
            // Progress at least every 16 MiB, every 250 ms, and at the end.
            if last_tick.elapsed() >= Duration::from_millis(250)
                || rel >= desc.length
                || chunk % 4 == 3
            {
                let millis = u64::try_from(last_tick.elapsed().as_millis())
                    .unwrap_or(u64::MAX)
                    .max(1);
                rate = tick_bytes.saturating_mul(1000) / millis;
                sink(EraseEvent::Progress {
                    pass: pass_index + 1,
                    passes: passes.len(),
                    done: rel,
                    total: desc.length,
                    bytes_per_sec: rate,
                    unwritable: unwritable.iter().map(|r| r.length).sum(),
                });
                last_tick = Instant::now();
                tick_bytes = 0;
            }
            if last_save.elapsed() >= Duration::from_secs(5) {
                save_resume(
                    request.state_dir.as_ref(),
                    &ResumePoint {
                        key: key.clone(),
                        method: request.method.id(),
                        pass: pass_index,
                        offset: rel,
                        seed: seed.to_hex(),
                        bytes_written,
                        unwritable: unwritable.clone(),
                    },
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

    certificate.passes_completed = passes_completed;
    certificate.bytes_written = bytes_written;
    certificate.unwritable = unwritable.clone();

    let mut verification = None;
    if outcome.is_ok() {
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
        clear_resume(request.state_dir.as_ref(), &key);
    }
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
        &certificate,
    );
    tracing::info!(verdict = %summary.verdict, seconds = summary.seconds, "erase finished");
    match (&outcome, resume_point) {
        (Ok(()), _) => sink(EraseEvent::Finished {
            summary: summary.clone(),
        }),
        (Err(EraseError::Cancelled), Some(resume)) => sink(EraseEvent::Cancelled {
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
