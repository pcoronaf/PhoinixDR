//! `phoinix read` — dump raw bytes from a source.

use std::io::Write;
use std::path::PathBuf;

use anyhow::Context;
use phoinix_block::{BlockReaderExt, MAX_SINGLE_READ};
use phoinix_core::fmt::hex_dump;
use phoinix_device::open_source;

/// Arguments for `phoinix read`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Device path or image file.
    source: PathBuf,
    /// Byte offset to start reading at.
    #[arg(long, default_value_t = 0)]
    offset: u64,
    /// Number of bytes to read (at most 16 MiB).
    #[arg(long, default_value_t = 512)]
    length: usize,
    /// Print a hex dump instead of raw bytes.
    #[arg(long)]
    hex: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    anyhow::ensure!(
        args.length <= MAX_SINGLE_READ,
        "length exceeds the {MAX_SINGLE_READ}-byte limit"
    );
    let source =
        open_source(&args.source).with_context(|| format!("opening {}", args.source.display()))?;
    let bytes = source
        .read_vec(args.offset, args.length)
        .context("reading")?;
    let mut stdout = std::io::stdout().lock();
    if args.hex {
        stdout.write_all(hex_dump(args.offset, &bytes).as_bytes())?;
    } else {
        stdout.write_all(&bytes)?;
    }
    Ok(())
}
