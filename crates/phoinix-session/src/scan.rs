//! The scan job: metadata walk, optional carving, deduplication, with
//! progress events, cancellation and partial results.

use std::sync::atomic::{AtomicBool, Ordering};

use phoinix_carve::{CarveEngine, CarveOptions, SignatureSet};
use phoinix_fs::RecoveryCandidate;
use phoinix_health::CandidateSource;

use crate::SessionError;
use crate::dto::{CandidateSummary, ScanEvent, ScanMode, ScanPhase, ScanRequest};
use crate::session::{ScanSession, now};
use crate::source::{OpenVolume, open_volume};

/// Candidates per `ScanEvent::Candidates` batch.
pub const BATCH: usize = 64;

/// What a scan produced: the (possibly partial) session and the outcome.
#[derive(Debug)]
pub struct ScanOutcome {
    /// The session, with whatever was found; `None` only when the source
    /// could not be opened.
    pub session: Option<ScanSession>,
    /// `Ok` when the scan ran to the end; [`SessionError::Cancelled`] when
    /// it was cancelled (partial results in `session`).
    pub result: Result<(), SessionError>,
}

/// Runs a scan, sending events to `sink`; `cancel` stops it between
/// candidates or chunks. The returned session holds whatever was found,
/// with `complete` set only when the scan ran to the end. Failures after
/// the scan started are also reported as [`ScanEvent::Failed`], and
/// cancellation as [`ScanEvent::Cancelled`].
pub fn run_scan(
    request: &ScanRequest,
    sink: &mut dyn FnMut(ScanEvent),
    cancel: &AtomicBool,
) -> ScanOutcome {
    sink(ScanEvent::Phase {
        phase: ScanPhase::Opening,
    });
    let volume = match open_volume(&request.source, request.partition, request.examine_content) {
        Ok(v) => v,
        Err(e) => {
            sink(ScanEvent::Failed {
                message: e.to_string(),
            });
            return ScanOutcome {
                session: None,
                result: Err(e),
            };
        }
    };
    let mut session = ScanSession::new(request.source.clone(), volume.info.clone(), request.mode);
    sink(ScanEvent::Started {
        session_id: session.id.clone(),
        filesystem: volume.info.filesystem,
        volume: volume.info.clone(),
    });
    let result = scan_into(&volume, request, &mut session, sink, cancel);
    session.finished = Some(now());
    match &result {
        Ok(()) => {
            session.complete = true;
            sink(ScanEvent::Finished {
                summary: session.summary(),
            });
        }
        Err(SessionError::Cancelled) => sink(ScanEvent::Cancelled {
            summary: session.summary(),
        }),
        Err(e) => sink(ScanEvent::Failed {
            message: e.to_string(),
        }),
    }
    ScanOutcome {
        session: Some(session),
        result,
    }
}

fn scan_into(
    volume: &OpenVolume,
    request: &ScanRequest,
    session: &mut ScanSession,
    sink: &mut dyn FnMut(ScanEvent),
    cancel: &AtomicBool,
) -> Result<(), SessionError> {
    if volume.engine.is_none() && request.mode == ScanMode::Quick {
        return Err(SessionError::Invalid(format!(
            "no undelete engine for {}; run a deep scan to carve the raw volume",
            volume.info.filesystem
        )));
    }
    // ---- metadata --------------------------------------------------------
    if let Some(engine) = volume.engine.as_deref() {
        sink(ScanEvent::Phase {
            phase: ScanPhase::Metadata,
        });
        let mut batch: Vec<CandidateSummary> = Vec::new();
        let mut seen = 0u64;
        for item in engine.deleted_files() {
            if cancel.load(Ordering::Relaxed) {
                flush(&mut batch, sink);
                return Err(SessionError::Cancelled);
            }
            seen += 1;
            match item {
                Ok(c) => {
                    batch.push(CandidateSummary::from_candidate(&c));
                    session.candidates.push(c);
                    if batch.len() >= BATCH {
                        flush(&mut batch, sink);
                        sink(ScanEvent::Progress {
                            phase: ScanPhase::Metadata,
                            done: seen,
                            total: None,
                            candidates: session.candidates.len() as u64,
                        });
                    }
                }
                Err(e) => tracing::warn!(error = %e, "candidate skipped"),
            }
        }
        flush(&mut batch, sink);
        sink(ScanEvent::Progress {
            phase: ScanPhase::Metadata,
            done: seen,
            total: Some(seen),
            candidates: session.candidates.len() as u64,
        });
    }
    // ---- carving ---------------------------------------------------------
    if request.mode == ScanMode::Deep {
        sink(ScanEvent::Phase {
            phase: ScanPhase::Carving,
        });
        let mut signatures = SignatureSet::builtin();
        if !request.carve.types.is_empty() {
            signatures = signatures.only(&request.carve.types)?;
        }
        let mut options = CarveOptions {
            whole_volume: request.carve.whole_volume || volume.engine.is_none(),
            min_size: request.carve.min_size,
            examine_content: request.examine_content,
            ..Default::default()
        };
        if request.carve.alignment > 0 {
            options.scan.alignment = request.carve.alignment;
        }
        let carver = CarveEngine::new(
            volume.reader.clone(),
            volume.space.clone(),
            volume.info.filesystem,
            volume.storage.clone(),
        )
        .with_signatures(signatures)
        .with_options(options);
        let base = session.candidates.len() as u64;
        let mut cancelled = false;
        let (carved, mut report) = carver.carve(&mut |p| {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
            }
            sink(ScanEvent::Progress {
                phase: ScanPhase::Carving,
                done: p.bytes_scanned,
                total: Some(p.bytes_total),
                candidates: base + p.hits as u64,
            });
        })?;
        if cancelled || cancel.load(Ordering::Relaxed) {
            return Err(SessionError::Cancelled);
        }
        sink(ScanEvent::Phase {
            phase: ScanPhase::Finishing,
        });
        let (carved, merged) = match volume.engine.as_deref() {
            Some(engine) => {
                let extents_of = |c: &RecoveryCandidate| engine.content_extents(c).ok();
                CarveEngine::deduplicate(carved, &mut session.candidates, &extents_of)
            }
            None => (carved, 0),
        };
        report.merged_into_metadata = merged;
        session.carving = Some(report);
        for chunk in carved.chunks(BATCH) {
            sink(ScanEvent::Candidates {
                items: chunk.iter().map(CandidateSummary::from_candidate).collect(),
            });
        }
        session.candidates.extend(carved);
        sink(ScanEvent::Progress {
            phase: ScanPhase::Finishing,
            done: 1,
            total: Some(1),
            candidates: session.candidates.len() as u64,
        });
    }
    debug_assert!(
        session
            .candidates
            .iter()
            .filter(|c| c.evidence.source == CandidateSource::FileCarving)
            .count()
            <= session.candidates.len()
    );
    Ok(())
}

fn flush(batch: &mut Vec<CandidateSummary>, sink: &mut dyn FnMut(ScanEvent)) {
    if !batch.is_empty() {
        sink(ScanEvent::Candidates {
            items: std::mem::take(batch),
        });
    }
}
