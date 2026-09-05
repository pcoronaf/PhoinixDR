//! `phoinix partitions` — find volumes by their filesystem structures,
//! independently of the partition table (lost-partition recovery).

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;

use phoinix_block::BlockReader;
use phoinix_carve::ScanProgress;
use phoinix_core::fmt::{bytes_si, grouped};
use phoinix_partition_recovery::{
    FoundVia, PartitionCandidate, Relation, SearchOptions, find_partitions,
};
use phoinix_volume::PartitionTable;
use serde::Serialize;

use crate::output::{self, outln};
use crate::source;

/// Arguments for `phoinix partitions`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Device path or image file.
    source: PathBuf,
    /// Do not open the candidates with their filesystem engines (faster).
    #[arg(long)]
    no_verify: bool,
    /// Test only offsets that are multiples of this (default 512).
    #[arg(long, default_value_t = 512)]
    align: u64,
    /// Emit JSON.
    #[arg(long)]
    json: bool,
}

/// Runs the search, showing progress on stderr when it is a terminal.
pub fn search_with_progress(
    reader: &Arc<dyn BlockReader>,
    table: Option<&PartitionTable>,
    options: &SearchOptions,
) -> anyhow::Result<Vec<PartitionCandidate>> {
    let enabled = std::io::stderr().is_terminal();
    let mut last = u64::MAX;
    let mut progress = |p: &ScanProgress| {
        if !enabled || p.bytes_total == 0 {
            return;
        }
        let percent = p.bytes_scanned.saturating_mul(100) / p.bytes_total;
        if percent != last {
            last = percent;
            let mut err = std::io::stderr().lock();
            let _ = write!(
                err,
                "\rPartition search: {percent}% ({} of {})   ",
                bytes_si(p.bytes_scanned),
                bytes_si(p.bytes_total)
            );
            let _ = err.flush();
        }
    };
    let found = find_partitions(reader, table, options, &mut progress)?;
    if enabled && last != u64::MAX {
        let _ = writeln!(std::io::stderr().lock());
    }
    Ok(found)
}

#[derive(Serialize)]
struct Report<'a> {
    source: &'a std::path::Path,
    size: u64,
    scheme: String,
    candidates: &'a [PartitionCandidate],
}

fn relation_text(r: &Relation) -> String {
    match r {
        Relation::Listed { index } => format!("partition {index}"),
        Relation::Lost => "LOST".into(),
        Relation::InsidePartition { index } => format!("inside partition {index}"),
        Relation::Nested { within } => format!("nested in #{}", within + 1),
        Relation::Overlapping { with } => format!("overlaps #{}", with + 1),
    }
}

fn found_via_text(f: FoundVia) -> &'static str {
    match f {
        FoundVia::PrimaryBootSector => "boot sector",
        FoundVia::BackupBootSector => "backup boot sector",
        FoundVia::Superblock => "superblock",
        FoundVia::BackupSuperblock => "backup superblock",
    }
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let opened = source::open(&args.source)?;
    let options = SearchOptions {
        alignment: args.align.max(1),
        verify: !args.no_verify,
        ..Default::default()
    };
    let candidates = search_with_progress(&opened.reader, Some(&opened.table), &options)?;
    if args.json {
        return output::print_json(&Report {
            source: &args.source,
            size: opened.reader.len(),
            scheme: format!("{:?}", opened.table.scheme),
            candidates: &candidates,
        });
    }
    if candidates.is_empty() {
        outln!(
            "No filesystem structures found on {}.",
            args.source.display()
        );
        return Ok(());
    }
    let sector = u64::from(opened.reader.geometry().logical_sector_size.max(1));
    let rows: Vec<Vec<String>> = candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            vec![
                format!("#{}", i + 1),
                format!("{} (LBA {})", grouped(c.start), grouped(c.start / sector)),
                bytes_si(c.length),
                c.filesystem.to_string(),
                c.label.clone().unwrap_or_default(),
                found_via_text(c.found_via).to_owned(),
                relation_text(&c.relation),
                format!("{}%", c.confidence),
            ]
        })
        .collect();
    output::write_raw(&output::table(
        &[
            "ID",
            "START",
            "SIZE",
            "FS",
            "LABEL",
            "FOUND VIA",
            "STATUS",
            "CONF",
        ],
        &rows,
    ));
    for (i, c) in candidates.iter().enumerate() {
        outln!();
        outln!("#{} {}", i + 1, c.describe());
        for e in &c.evidence {
            outln!(
                "   {} {}",
                if e.supports { "✓" } else { "⚠" },
                e.description
            );
        }
        for r in &c.repairs {
            outln!("   ↺ on mount: {}", r.description);
        }
    }
    outln!();
    let lost = candidates
        .iter()
        .filter(|c| matches!(c.relation, Relation::Lost))
        .count();
    outln!(
        "{} candidate(s), {} not in the partition table. Nothing was written. Browse or recover a candidate with `phoinix scan {} --lost <ID>` (or `--at <START>`).",
        candidates.len(),
        lost,
        args.source.display()
    );
    Ok(())
}
