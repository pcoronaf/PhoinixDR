//! `phoinix scan` — list recoverable files with their health.

use phoinix_core::fmt::bytes_iec;
use phoinix_fs::RecoveryCandidate;
use phoinix_health::HealthCategory;

use crate::commands::undelete::{Session, SourceArgs};
use crate::output::{self, outln};

/// Arguments for `phoinix scan`.
#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(flatten)]
    source: SourceArgs,
    /// Scan for deleted files (the only mode implemented so far; implied).
    #[arg(long)]
    deleted: bool,
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

pub fn run(args: Args) -> anyhow::Result<()> {
    let session = Session::open(&args.source)?;
    let engine = &*session.engine;
    let needle = args.name.as_ref().map(|n| n.to_lowercase());
    let mut candidates: Vec<RecoveryCandidate> = Vec::new();
    for item in engine.deleted_files() {
        let c = match item {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "candidate skipped");
                continue;
            }
        };
        if let Some(min) = args.min_health
            && (c.health.category == HealthCategory::Unknown || c.health.category < min)
        {
            continue;
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
            continue;
        }
        candidates.push(c);
    }
    if args.json {
        return output::print_json(&candidates);
    }
    if candidates.is_empty() {
        outln!("No deleted files found.");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = candidates
        .iter()
        .map(|c| {
            vec![
                c.filesystem_object.short_reference(),
                c.display_name(),
                c.logical_size.map_or_else(|| "-".to_owned(), bytes_iec),
                format!("{} {}", c.health.category, c.health.likelihood),
                c.health.confidence.to_string(),
                c.original_path.clone().unwrap_or_default(),
            ]
        })
        .collect();
    output::write_raw(&output::table(
        &["ID", "NAME", "SIZE", "RECOVERY", "CONF", "PATH"],
        &rows,
    ));
    outln!();
    outln!(
        "{} candidate(s) on the {} volume. Recovery figures are estimates. Use `phoinix explain <source> <ID>` for the evidence.",
        candidates.len(),
        session.filesystem
    );
    Ok(())
}
