//! `phoinix verify` — hash a source and compare with the hashes stored in
//! its image container.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use phoinix_core::fmt::{bytes_si, grouped};
use phoinix_image::{HashVerification, StoredHashes};
use serde::Serialize;

use crate::commands::inspect::print_container;
use crate::output::{self, outln};
use crate::source;

/// Arguments for `phoinix verify`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Device path or image file.
    source: PathBuf,
    /// Emit JSON instead of text.
    #[arg(long)]
    json: bool,
    /// Do not print progress.
    #[arg(long)]
    quiet: bool,
}

#[derive(Serialize)]
struct Report {
    path: String,
    container: Option<phoinix_image::ContainerInfo>,
    verification: HashVerification,
}

/// Hashes `reader`, printing progress to stderr unless `quiet` or stderr
/// is not a terminal.
pub fn hash_source(
    reader: &dyn phoinix_block::BlockReader,
    stored: &StoredHashes,
    quiet: bool,
) -> anyhow::Result<HashVerification> {
    let show_progress = !quiet && std::io::stderr().is_terminal();
    let mut last = 0u64;
    let verification = phoinix_image::verify(reader, stored, &mut |done, total| {
        if show_progress && (done - last >= 64 * 1024 * 1024 || done == total) {
            last = done;
            let pct = done.saturating_mul(100).checked_div(total).unwrap_or(100);
            eprint!(
                "\rhashing {} of {} ({pct}%)",
                bytes_si(done),
                bytes_si(total)
            );
            let _ = std::io::stderr().flush();
        }
        true
    })?;
    if show_progress {
        eprintln!();
    }
    Ok(verification)
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let opened = source::open(&args.source)?;
    let container = opened
        .container
        .clone()
        .filter(phoinix_image::ContainerInfo::is_container);
    let stored = container
        .as_ref()
        .map(|c| c.stored_hashes.clone())
        .unwrap_or_default();
    let verification = hash_source(&*opened.reader, &stored, args.quiet || args.json)?;
    let report = Report {
        path: args.source.display().to_string(),
        container,
        verification,
    };
    if args.json {
        output::print_json(&report)?;
    } else {
        if let Some(c) = &report.container {
            print_container(c);
            outln!();
        }
        let v = &report.verification;
        outln!(
            "Hashes over {} bytes ({})",
            grouped(v.bytes),
            bytes_si(v.bytes)
        );
        outln!("  MD5:      {}", v.md5);
        outln!("  SHA-1:    {}", v.sha1);
        outln!("  SHA-256:  {}", v.sha256);
        match (v.md5_matches, v.sha1_matches) {
            (None, None) => outln!("  The container stores no hash to compare with."),
            (md5, sha1) => {
                if let Some(ok) = md5 {
                    outln!(
                        "  Stored MD5 {}",
                        if ok { "matches" } else { "DOES NOT MATCH" }
                    );
                }
                if let Some(ok) = sha1 {
                    outln!(
                        "  Stored SHA-1 {}",
                        if ok { "matches" } else { "DOES NOT MATCH" }
                    );
                }
            }
        }
    }
    anyhow::ensure!(
        report.verification.verified() != Some(false),
        "the computed hashes do not match the hashes stored in the image"
    );
    Ok(())
}
