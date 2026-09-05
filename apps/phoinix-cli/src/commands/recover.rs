//! `phoinix recover` — write candidates to a destination and verify.

use std::path::{Path, PathBuf};

use anyhow::Context;
use phoinix_core::fmt::grouped;
use phoinix_fs::{DeletedFileProvider, RecoveryCandidate};
use phoinix_recovery::{
    CaseMetadata, RecoveryReport, RecoveryRequest, RecoveryWriter, ReportSource, ReportVolume,
};
use serde::Serialize;

use crate::commands::undelete::{CarveArgs, Session, SourceArgs};
use crate::commands::verify::hash_source;
use crate::output::{self, outln};

/// Arguments for `phoinix recover`.
#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(flatten)]
    source: SourceArgs,
    /// Candidate references from `phoinix scan` (`<record>`,
    /// `<record>:<stream>`, or `c<offset>` for carved files).
    #[arg(required = true)]
    candidates: Vec<String>,
    #[command(flatten)]
    carve: CarveArgs,
    /// Destination directory (must not be on the source disk).
    #[arg(long, short = 'o')]
    output: PathBuf,
    /// Recreate the original directory tree under the destination.
    #[arg(long)]
    preserve_tree: bool,
    /// Do not apply original timestamps to recovered files.
    #[arg(long)]
    no_timestamps: bool,
    /// Skip SHA-256 computation.
    #[arg(long)]
    no_hash: bool,
    /// Overwrite existing files instead of choosing a new name.
    #[arg(long)]
    overwrite: bool,
    /// Expert override: allow a destination on the same disk as a device
    /// source. This can destroy the data you are trying to recover.
    #[arg(long)]
    allow_source_destination: bool,
    /// Write a recovery report (`.json`, `.md` or `.html` by extension).
    #[arg(long, value_name = "PATH")]
    report: Option<PathBuf>,
    /// Case number for the report (defaults to the image's acquisition header).
    #[arg(long, value_name = "TEXT")]
    case_number: Option<String>,
    /// Evidence number for the report.
    #[arg(long, value_name = "TEXT")]
    evidence_number: Option<String>,
    /// Examiner for the report.
    #[arg(long, value_name = "TEXT")]
    examiner: Option<String>,
    /// Notes for the report.
    #[arg(long, value_name = "TEXT")]
    case_notes: Option<String>,
    /// Hash the whole source and record the verification in the report.
    #[arg(long)]
    verify_source: bool,
    /// Emit JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Serialize)]
struct Report {
    candidate: String,
    name: String,
    result: Option<phoinix_recovery::RecoveryResult>,
    error: Option<String>,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let any_carved = args
        .candidates
        .iter()
        .any(|r| Session::is_carved_reference(r));
    let all_carved = args
        .candidates
        .iter()
        .all(|r| Session::is_carved_reference(r));
    let session = if all_carved {
        Session::open_any(&args.source)?
    } else {
        Session::open(&args.source)?
    };
    let mut request = RecoveryRequest::new(&args.output);
    request.preserve_tree = args.preserve_tree;
    request.preserve_timestamps = !args.no_timestamps;
    request.hash_after_write = !args.no_hash;
    request.overwrite = args.overwrite;
    request.allow_same_device = args.allow_source_destination;
    // One writer per engine: metadata candidates and carved candidates are
    // resolved by different providers over the same volume.
    let carver = if any_carved {
        Some(session.carve_engine(&args.carve)?)
    } else {
        None
    };
    let metadata_writer = match session.engine.as_deref() {
        Some(engine) => Some(RecoveryWriter::new(
            engine,
            &args.source.source,
            request.clone(),
        )?),
        None => None,
    };
    let carve_writer = match &carver {
        Some(c) => Some(RecoveryWriter::new(c, &args.source.source, request)?),
        None => None,
    };
    if let Some(w) = metadata_writer
        .as_ref()
        .or(carve_writer.as_ref())
        .and_then(|w| w.destination_check().warning())
    {
        eprintln!("warning: {w}");
    }

    let mut reports = Vec::new();
    let mut entries: Vec<(Option<RecoveryCandidate>, Option<String>)> = Vec::new();
    let mut failures = 0usize;
    for reference in &args.candidates {
        let (engine, writer): (&dyn DeletedFileProvider, &RecoveryWriter<'_>) =
            if Session::is_carved_reference(reference) {
                match (&carver, &carve_writer) {
                    (Some(c), Some(w)) => (c, w),
                    _ => continue,
                }
            } else {
                match (session.engine.as_deref(), &metadata_writer) {
                    (Some(e), Some(w)) => (e, w),
                    _ => {
                        failures += 1;
                        let error = format!(
                            "no undelete engine for {}; only carved references (c<offset>) can be recovered here",
                            session.filesystem
                        );
                        entries.push((None, Some(error.clone())));
                        reports.push(Report {
                            candidate: reference.clone(),
                            name: String::new(),
                            result: None,
                            error: Some(error),
                        });
                        continue;
                    }
                }
            };
        let object = match engine.object_from_reference(reference) {
            Ok(o) => o,
            Err(e) => {
                failures += 1;
                entries.push((None, Some(e.to_string())));
                reports.push(Report {
                    candidate: reference.clone(),
                    name: String::new(),
                    result: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };
        let candidate = match engine.candidate(&object) {
            Ok(c) => c,
            Err(e) => {
                failures += 1;
                entries.push((None, Some(e.to_string())));
                reports.push(Report {
                    candidate: reference.clone(),
                    name: String::new(),
                    result: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };
        let name = candidate.display_name();
        match writer.recover(&candidate) {
            Ok(result) => {
                if !result.complete {
                    failures += 1;
                }
                entries.push((Some(candidate), None));
                reports.push(Report {
                    candidate: reference.clone(),
                    name,
                    result: Some(result),
                    error: None,
                });
            }
            Err(e) => {
                failures += 1;
                entries.push((Some(candidate), Some(e.to_string())));
                reports.push(Report {
                    candidate: reference.clone(),
                    name,
                    result: None,
                    error: Some(e.to_string()),
                });
            }
        }
    }
    let report_path = match &args.report {
        Some(path) => Some(write_report(&args, &session, &reports, &entries, path)?),
        None => None,
    };
    if args.json {
        output::print_json(&reports)?;
    } else {
        if let Some(p) = &report_path {
            outln!("Report written to {}", p.display());
        }
        for r in &reports {
            match (&r.result, &r.error) {
                (Some(res), _) => {
                    outln!(
                        "{}  {}  {} bytes{}  → {}",
                        r.candidate,
                        r.name,
                        grouped(res.bytes_written),
                        if res.complete { "" } else { "  PARTIAL" },
                        res.output_path.display()
                    );
                    if let Some(h) = &res.sha256 {
                        outln!("    SHA-256 {h}");
                    }
                    for d in &res.diagnostics {
                        outln!("    ⚠ {}", d.message);
                    }
                }
                (None, Some(err)) => outln!("{}  {}  FAILED: {err}", r.candidate, r.name),
                (None, None) => {}
            }
        }
    }
    anyhow::ensure!(
        failures == 0,
        "{failures} of {} recoveries failed or were partial",
        reports.len()
    );
    Ok(())
}

/// Builds and writes the recovery report.
fn write_report(
    args: &Args,
    session: &Session,
    reports: &[Report],
    entries: &[(Option<RecoveryCandidate>, Option<String>)],
    path: &Path,
) -> anyhow::Result<PathBuf> {
    let verification = if args.verify_source {
        let stored = session
            .container
            .as_ref()
            .map(|c| c.stored_hashes.clone())
            .unwrap_or_default();
        let source = crate::source::open(&args.source.source)?;
        Some(hash_source(&*source.reader, &stored, args.json)?)
    } else {
        None
    };
    let case = CaseMetadata {
        case_number: args.case_number.clone(),
        evidence_number: args.evidence_number.clone(),
        examiner: args.examiner.clone(),
        notes: args.case_notes.clone(),
    }
    .with_acquisition_defaults(
        session
            .container
            .as_ref()
            .and_then(|c| c.acquisition.as_ref()),
    );
    let mut report = RecoveryReport::new(
        case,
        ReportSource {
            path: args.source.source.display().to_string(),
            size: session.source_len,
            sector_size: session.reader.geometry().logical_sector_size,
            is_device: session.container.is_none(),
            container: session
                .container
                .clone()
                .filter(phoinix_image::ContainerInfo::is_container),
            verification,
        },
        Some(ReportVolume {
            partition: session.partition,
            offset: session.volume_offset,
            length: session.reader.len(),
            filesystem: session.filesystem.to_string(),
        }),
        &args.output,
    );
    for (r, (candidate, error)) in reports.iter().zip(entries) {
        match (&r.result, error) {
            (Some(result), _) => report.push(&r.candidate, candidate.as_ref(), Ok(result)),
            (None, Some(e)) => report.push(&r.candidate, candidate.as_ref(), Err(e)),
            (None, None) => report.push(&r.candidate, candidate.as_ref(), Err("not written")),
        }
    }
    report
        .write_to(path)
        .with_context(|| format!("writing the report to {}", path.display()))
}
