//! Recovery through the service layer: candidates of a session are written
//! to a destination by the engines of a freshly opened volume.

use std::path::Path;

use phoinix_carve::CarveEngine;
use phoinix_fs::{DeletedFileProvider, RecoveryCandidate};
use phoinix_health::CandidateSource;
use phoinix_image::HashVerification;
use phoinix_recovery::{
    RecoveryReport, RecoveryRequest, RecoveryWriter, ReportSource, ReportVolume, check_destination,
};

use crate::SessionError;
use crate::dto::{DestinationInfo, RecoverEvent, RecoverItem, RecoverRequest};
use crate::session::ScanSession;
use crate::source::{VolumeChoice, container_of, is_device_path, open_volume_with};

/// Assesses a destination for `source`.
#[must_use]
pub fn destination_info(source: &Path, destination: &Path) -> DestinationInfo {
    let check = check_destination(source, destination);
    DestinationInfo {
        destination: destination.to_path_buf(),
        same_disk: check.same_disk,
        overwrites_source_image: check.overwrites_source_image,
        dangerous: check.is_dangerous(),
        warning: check.warning(),
    }
}

/// Recovers the requested candidates of `session`, sending events to
/// `sink`. Returns the outcomes.
///
/// # Errors
///
/// Returns [`SessionError`] if the source cannot be reopened or the
/// destination is refused; per-candidate failures are reported in the
/// items instead.
pub fn recover(
    session: &ScanSession,
    request: &RecoverRequest,
    sink: &mut dyn FnMut(RecoverEvent),
) -> Result<Vec<RecoverItem>, SessionError> {
    let candidates: Vec<&RecoveryCandidate> = request
        .candidates
        .iter()
        .map(|id| {
            session.candidate(*id).ok_or_else(|| {
                SessionError::NotFound(format!("candidate {id} is not in this session"))
            })
        })
        .collect::<Result<_, _>>()?;
    let volume = open_volume_with(
        &session.source,
        &VolumeChoice::reopen(&session.volume),
        false,
    )?;
    let mut req = RecoveryRequest::new(&request.destination);
    req.preserve_tree = request.preserve_tree;
    req.preserve_timestamps = request.preserve_timestamps;
    req.hash_after_write = request.hash;
    req.overwrite = request.overwrite;
    req.allow_same_device = request.allow_same_device;

    let needs_carver = candidates
        .iter()
        .any(|c| c.evidence.source == CandidateSource::FileCarving);
    let carver = needs_carver.then(|| {
        CarveEngine::new(
            volume.reader.clone(),
            volume.space.clone(),
            volume.info.filesystem,
            volume.storage.clone(),
        )
    });
    let metadata_writer = match volume.engine.as_deref() {
        Some(engine) => Some(RecoveryWriter::new(engine, &session.source, req.clone())?),
        None => None,
    };
    let carve_writer = match &carver {
        Some(c) => Some(RecoveryWriter::new(c, &session.source, req)?),
        None => None,
    };
    let warning = metadata_writer
        .as_ref()
        .or(carve_writer.as_ref())
        .and_then(|w| w.destination_check().warning());
    let total = candidates.len();
    sink(RecoverEvent::Started { total, warning });
    // The report, when requested, describes the source as it is now.
    let container = if request.report.is_some() || request.case.is_some() {
        session
            .container
            .clone()
            .or_else(|| container_of(&session.source))
    } else {
        None
    };
    let verification: Option<HashVerification> = if request.verify_source {
        let stored = container
            .as_ref()
            .map(|c| c.stored_hashes.clone())
            .unwrap_or_default();
        let source = crate::source::open(&session.source)?;
        Some(phoinix_image::verify(
            &*source.reader,
            &stored,
            &mut |done, total| {
                sink(RecoverEvent::Verifying { done, total });
                true
            },
        )?)
    } else {
        None
    };
    let mut report = request.report.as_ref().map(|_| {
        RecoveryReport::new(
            request
                .case
                .clone()
                .unwrap_or_default()
                .with_acquisition_defaults(container.as_ref().and_then(|c| c.acquisition.as_ref())),
            ReportSource {
                path: session.source.display().to_string(),
                size: container.as_ref().map_or(volume.reader.len(), |c| c.size),
                sector_size: volume.reader.geometry().logical_sector_size,
                is_device: is_device_path(&session.source),
                container: container.clone(),
                verification,
            },
            Some(ReportVolume {
                partition: session.volume.partition,
                offset: session.volume.offset,
                length: session.volume.length,
                filesystem: session.volume.filesystem.to_string(),
            }),
            &request.destination,
        )
    });
    let mut items = Vec::new();
    let mut failures = 0usize;
    for (i, c) in candidates.iter().enumerate() {
        let writer = if c.evidence.source == CandidateSource::FileCarving {
            carve_writer.as_ref()
        } else {
            metadata_writer.as_ref()
        };
        let item = match writer {
            Some(w) => match w.recover(c) {
                Ok(result) => {
                    if !result.complete {
                        failures += 1;
                    }
                    RecoverItem {
                        id: c.id,
                        name: c.display_name(),
                        result: Some(result),
                        error: None,
                    }
                }
                Err(e) => {
                    failures += 1;
                    RecoverItem {
                        id: c.id,
                        name: c.display_name(),
                        result: None,
                        error: Some(e.to_string()),
                    }
                }
            },
            None => {
                failures += 1;
                RecoverItem {
                    id: c.id,
                    name: c.display_name(),
                    result: None,
                    error: Some(format!(
                        "no engine can read this candidate on {}",
                        volume.info.filesystem
                    )),
                }
            }
        };
        if let Some(r) = report.as_mut() {
            let reference = c.filesystem_object.short_reference();
            match (&item.result, &item.error) {
                (Some(res), _) => r.push(&reference, Some(c), Ok(res)),
                (None, Some(e)) => r.push(&reference, Some(c), Err(e)),
                (None, None) => r.push(&reference, Some(c), Err("not attempted")),
            }
        }
        sink(RecoverEvent::Item {
            index: i + 1,
            total,
            item: item.clone(),
        });
        items.push(item);
    }
    let written = match (&report, &request.report) {
        (Some(r), Some(path)) => Some(r.write_to(path).map_err(|e| {
            SessionError::Invalid(format!("writing the report to {}: {e}", path.display()))
        })?),
        _ => None,
    };
    sink(RecoverEvent::Finished {
        items: items.clone(),
        failures,
        report: written,
    });
    Ok(items)
}

/// Whether `provider` can serve `candidate` (used by tests and callers that
/// hold engines themselves).
#[must_use]
pub fn serves(provider: &dyn DeletedFileProvider, candidate: &RecoveryCandidate) -> bool {
    provider.content_extents(candidate).is_ok()
}
