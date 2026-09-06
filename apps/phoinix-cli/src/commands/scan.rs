//! `phoinix scan` — list recoverable files with their health.
//!
//! The quick scan walks filesystem metadata. `--deep` adds signature
//! carving of the unallocated space (or the whole volume with
//! `--carve-all`), deduplicated against the metadata candidates.

use std::io::{IsTerminal, Write};

use phoinix_carve::{CarveReport, CarveStage, ScanProgress};
use phoinix_core::fmt::{bytes_iec, bytes_si};
use phoinix_fs::RecoveryCandidate;
use phoinix_health::{CandidateSource, HealthCategory};
use serde::Serialize;

use crate::commands::undelete::{CarveArgs, Session, SourceArgs};
use crate::output::{self, outln};

/// Arguments for `phoinix scan`.
#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(flatten)]
    source: SourceArgs,
    /// Scan for deleted files through filesystem metadata (the default;
    /// implied).
    #[arg(long)]
    deleted: bool,
    /// Deep scan: also carve files by signature from the unallocated space.
    #[arg(long)]
    deep: bool,
    /// Only carve; skip the metadata scan.
    #[arg(long, requires = "deep")]
    carve_only: bool,
    #[command(flatten)]
    carve: CarveArgs,
    /// Only show candidates at or above this health category
    /// (excellent, very-good, good, poor, very-poor).
    #[arg(long, value_parser = parse_category)]
    min_health: Option<HealthCategory>,
    /// Only show candidates whose name or path contains this text
    /// (case-insensitive).
    #[arg(long)]
    name: Option<String>,
    /// Emit JSON.
    #[arg(long)]
    json: bool,
}

fn parse_category(text: &str) -> Result<HealthCategory, String> {
    HealthCategory::parse(text).ok_or_else(|| format!("unknown health category {text:?}"))
}

#[derive(Serialize)]
struct JsonOutput<'a> {
    filesystem: String,
    candidates: &'a [RecoveryCandidate],
    #[serde(skip_serializing_if = "Option::is_none")]
    carving: Option<CarveReport>,
}

/// Reports deep-scan progress on stderr when it is a terminal.
struct Progress {
    enabled: bool,
    last_percent: u64,
}

impl Progress {
    fn new() -> Self {
        Self {
            enabled: std::io::stderr().is_terminal(),
            last_percent: u64::MAX,
        }
    }

    fn update(&mut self, p: &ScanProgress) {
        if !self.enabled {
            return;
        }
        let mut err = std::io::stderr().lock();
        match p.stage {
            CarveStage::Search => {
                if p.bytes_total == 0 {
                    return;
                }
                let percent = p.bytes_scanned.saturating_mul(100) / p.bytes_total;
                if percent == self.last_percent {
                    return;
                }
                self.last_percent = percent;
                let _ = write!(
                    err,
                    "\rDeep scan: header search {percent}% ({} of {}), {} hit(s)   ",
                    bytes_si(p.bytes_scanned),
                    bytes_si(p.bytes_total),
                    p.hits
                );
            }
            CarveStage::Assemble => {
                // A new line for the second stage, then in-place updates.
                if self.last_percent != u64::MAX - 1 {
                    let _ = writeln!(err);
                    self.last_percent = u64::MAX - 1;
                }
                let _ = write!(
                    err,
                    "\rDeep scan: examining hit {} of {}, {} file(s) assembled, {} read   ",
                    p.hits_done,
                    p.hits,
                    p.candidates,
                    bytes_si(p.bytes_read)
                );
            }
        }
        let _ = err.flush();
    }

    fn finish(&self) {
        if self.enabled && self.last_percent != u64::MAX {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err);
        }
    }
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let session = if args.deep {
        Session::open_any(&args.source)?
    } else {
        Session::open(&args.source)?
    };
    let mut candidates: Vec<RecoveryCandidate> = Vec::new();
    let mut from_metadata = 0usize;
    if !args.carve_only
        && let Some(engine) = session.engine.as_deref()
    {
        for item in engine.deleted_files() {
            match item {
                Ok(c) => candidates.push(c),
                Err(e) => tracing::warn!(error = %e, "candidate skipped"),
            }
        }
        from_metadata = candidates.len();
    }
    if session.storage.rotational == Some(false) {
        outln!(
            "Note: {} is a solid-state drive. Data deleted from an SSD is usually discarded (TRIM) within seconds and then reads as zeros; recovery is only possible when TRIM was not in effect (a USB enclosure, TRIM disabled, or data lost through a reformat). Zero-filled candidates are flagged as such.",
            args.source.source.display()
        );
    }
    let mut carving: Option<CarveReport> = None;
    if args.deep {
        let carver = session.carve_engine(&args.carve)?;
        let mut progress = Progress::new();
        let (carved, mut report) = carver.carve(&mut |p| progress.update(p))?;
        progress.finish();
        let (carved, merged) = match session.engine.as_deref() {
            Some(engine) if !args.carve_only => {
                let extents_of = |c: &RecoveryCandidate| engine.content_extents(c).ok();
                phoinix_carve::CarveEngine::deduplicate(carved, &mut candidates, &extents_of)
            }
            _ => (carved, 0),
        };
        report.merged_into_metadata = merged;
        candidates.extend(carved);
        carving = Some(report);
    }

    let needle = args.name.as_ref().map(|n| n.to_lowercase());
    candidates.retain(|c| {
        if let Some(min) = args.min_health
            && (c.health.category == HealthCategory::Unknown || c.health.category < min)
        {
            return false;
        }
        if let Some(n) = &needle
            && !c.display_name().to_lowercase().contains(n)
            && !c
                .original_path
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(n)
        {
            return false;
        }
        true
    });
    if args.json {
        return output::print_json(&JsonOutput {
            filesystem: session.filesystem.to_string(),
            candidates: &candidates,
            carving,
        });
    }
    if candidates.is_empty() {
        outln!("No recoverable files found.");
        if let Some(r) = &carving {
            outln!("{}", carving_summary(r));
        }
        return Ok(());
    }
    let rows: Vec<Vec<String>> = candidates
        .iter()
        .map(|c| {
            let carved = c.evidence.source == CandidateSource::FileCarving;
            vec![
                c.filesystem_object.short_reference(),
                c.display_name(),
                c.logical_size.map_or_else(|| "-".to_owned(), bytes_iec),
                format!("{} {}", c.health.category, c.health.likelihood),
                c.health.confidence.to_string(),
                if carved {
                    let name = c
                        .evidence
                        .content
                        .detected_type
                        .as_ref()
                        .map_or("unknown type", |t| t.name.as_str());
                    format!("(carved: {name})")
                } else {
                    c.original_path.clone().unwrap_or_default()
                },
            ]
        })
        .collect();
    output::write_raw(&output::table(
        &["ID", "NAME", "SIZE", "RECOVERY", "CONF", "PATH"],
        &rows,
    ));
    outln!();
    let carved_count = candidates
        .len()
        .saturating_sub(from_metadata.min(candidates.len()));
    match &carving {
        Some(r) => {
            outln!(
                "{} candidate(s) on the {} volume: {} from filesystem metadata, {} carved. {}",
                candidates.len(),
                session.filesystem,
                candidates.len().saturating_sub(carved_count),
                carved_count,
                carving_summary(r)
            );
        }
        None => outln!(
            "{} candidate(s) on the {} volume.",
            candidates.len(),
            session.filesystem
        ),
    }
    outln!("Recovery figures are estimates. Use `phoinix explain <source> <ID>` for the evidence.");
    outln!("{}", phoinix_core::DISCLAIMER);
    Ok(())
}

fn carving_summary(r: &CarveReport) -> String {
    let mut text = format!(
        "Deep scan covered {} of eligible space: {} header hit(s), {} nested hit(s) skipped, {} rejected, {} merged into filesystem candidates.",
        bytes_si(r.bytes_scanned),
        r.hits,
        r.nested_skipped,
        r.rejected + r.too_small,
        r.merged_into_metadata
    );
    if r.unreadable_bytes > 0 {
        text.push_str(&format!(
            " {} in {} region(s) could not be read from the device and were skipped (treated as zeros).",
            bytes_si(r.unreadable_bytes),
            r.unreadable_ranges
        ));
    }
    text
}
