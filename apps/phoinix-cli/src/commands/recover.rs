//! `phoinix recover` — write candidates to a destination and verify.

use std::path::PathBuf;

use phoinix_core::fmt::grouped;
use phoinix_recovery::{RecoveryRequest, RecoveryWriter};
use serde::Serialize;

use phoinix_fs::DeletedFileProvider;

use crate::commands::undelete::{CarveArgs, Session, SourceArgs};
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
                        reports.push(Report {
                            candidate: reference.clone(),
                            name: String::new(),
                            result: None,
                            error: Some(format!(
                                "no undelete engine for {}; only carved references (c<offset>) can be recovered here",
                                session.filesystem
                            )),
                        });
                        continue;
                    }
                }
            };
        let object = match engine.object_from_reference(reference) {
            Ok(o) => o,
            Err(e) => {
                failures += 1;
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
                reports.push(Report {
                    candidate: reference.clone(),
                    name,
                    result: Some(result),
                    error: None,
                });
            }
            Err(e) => {
                failures += 1;
                reports.push(Report {
                    candidate: reference.clone(),
                    name,
                    result: None,
                    error: Some(e.to_string()),
                });
            }
        }
    }
    if args.json {
        output::print_json(&reports)?;
    } else {
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
