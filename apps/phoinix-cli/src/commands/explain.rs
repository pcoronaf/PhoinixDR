//! `phoinix explain` — the evidence behind a candidate's score.

use phoinix_core::fmt::{bytes_iec, grouped};

use crate::commands::undelete::{Session, SourceArgs};
use crate::output::{self, outln};

/// Arguments for `phoinix explain`.
#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(flatten)]
    source: SourceArgs,
    /// Candidate reference from `phoinix scan` (`<record>` or `<record>:<stream>`).
    candidate: String,
    /// Emit JSON.
    #[arg(long)]
    json: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let session = Session::open(&args.source)?;
    let engine = &*session.engine;
    let object = engine.object_from_reference(&args.candidate)?;
    let c = engine.candidate(&object)?;
    if args.json {
        return output::print_json(&c);
    }
    outln!("{}", c.display_name());
    if let Some(p) = &c.original_path {
        outln!(
            "Original path:         {p}{}",
            if c.path_uncertain {
                "  (uncertain)"
            } else {
                ""
            }
        );
    }
    if let Some(s) = c.logical_size {
        outln!(
            "Size:                  {} ({} bytes)",
            bytes_iec(s),
            grouped(s)
        );
    }
    if let Some(m) = &c.timestamps.modified_iso {
        outln!("Modified:              {m}");
    }
    outln!("Object:                {}", c.filesystem_object);
    outln!();
    outln!(
        "Recovery likelihood:   {}% — {}",
        c.health.likelihood,
        c.health.category
    );
    outln!("Assessment confidence: {}%", c.health.confidence);
    outln!();
    outln!("Evidence:");
    for r in &c.health.reasons {
        outln!("  {} {}", if r.positive { "✓" } else { "⚠" }, r.text);
    }
    let e = &c.evidence;
    outln!();
    outln!("Details:");
    outln!(
        "  Extents: {}{}, {} fragment(s){}",
        if e.extents.resident {
            "resident"
        } else {
            "non-resident"
        },
        if e.extents.complete {
            ""
        } else {
            " (incomplete)"
        },
        e.extents.extent_count,
        e.extents
            .total_clusters
            .map_or(String::new(), |t| format!(", {} clusters", grouped(t)))
    );
    if e.allocation.map_available && e.allocation.clusters_total > 0 {
        outln!(
            "  Allocation: {} free, {} allocated, {} unknown of {} clusters",
            grouped(e.allocation.clusters_free),
            grouped(e.allocation.clusters_allocated),
            grouped(e.allocation.clusters_unknown),
            grouped(e.allocation.clusters_total)
        );
    }
    if let Some(t) = &e.content.detected_type {
        outln!("  Content type: {} (.{})", t.name, t.extension);
    }
    if let Some(v) = &e.content.validation {
        outln!("  Validation: {:?}", v.status);
        for check in &v.checks {
            outln!(
                "    {} {}: {}",
                if check.passed { "✓" } else { "✗" },
                check.name,
                check.detail
            );
        }
    }
    if let Some(z) = e.content.zero_block_ratio {
        outln!(
            "  Zero-filled sample blocks: {:.0}% of {} bytes examined",
            z * 100.0,
            grouped(e.content.bytes_examined)
        );
    }
    outln!(
        "  Storage: {:?}{}",
        e.storage.device_kind,
        match e.storage.rotational {
            Some(true) => ", rotational",
            Some(false) => ", solid state (TRIM state unknown)",
            None => "",
        }
    );
    Ok(())
}
