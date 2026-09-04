//! `phoinix ntfs …` — native NTFS reader utilities.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use phoinix_core::FileSystemType;
use phoinix_core::fmt::{bytes_iec, grouped, hex_dump};
use phoinix_fs_ntfs::{DataStorage, NtfsFile, NtfsVolume};
use serde::Serialize;

use crate::output::{self, outln};
use crate::source::{self, SelectedVolume};

/// Common source-selection arguments.
#[derive(Debug, clap::Args)]
pub struct SourceArgs {
    /// Device path or image file.
    source: PathBuf,
    /// Partition index to use (default: first NTFS partition, or the whole
    /// source when it has no partition table).
    #[arg(long, short = 'p')]
    partition: Option<u32>,
}

impl SourceArgs {
    fn open(&self) -> anyhow::Result<(SelectedVolume, NtfsVolume)> {
        let opened = source::open(&self.source)?;
        let selected = source::select_volume(&opened, self.partition, Some(FileSystemType::Ntfs))?;
        let volume = NtfsVolume::open(selected.reader.clone()).context("opening NTFS volume")?;
        Ok((selected, volume))
    }
}

/// NTFS subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Show volume geometry and metadata.
    Info(InfoArgs),
    /// List files (all MFT base records).
    Ls(LsArgs),
    /// Dump one MFT record.
    Record(RecordArgs),
    /// Extract a file's data stream by MFT record number.
    Extract(ExtractArgs),
}

/// Arguments for `ntfs info`.
#[derive(Debug, clap::Args)]
pub struct InfoArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// Emit JSON.
    #[arg(long)]
    json: bool,
}

/// Arguments for `ntfs ls`.
#[derive(Debug, clap::Args)]
pub struct LsArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// Include deleted (not in use) records.
    #[arg(long)]
    all: bool,
    /// Include NTFS metadata files (`$MFT`, `$Bitmap`, …).
    #[arg(long)]
    system: bool,
    /// Emit JSON.
    #[arg(long)]
    json: bool,
}

/// Arguments for `ntfs record`.
#[derive(Debug, clap::Args)]
pub struct RecordArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// MFT record number.
    record: u64,
    /// Also hex-dump the raw record.
    #[arg(long)]
    hex: bool,
    /// Emit JSON.
    #[arg(long)]
    json: bool,
}

/// Arguments for `ntfs extract`.
#[derive(Debug, clap::Args)]
pub struct ExtractArgs {
    #[command(flatten)]
    source: SourceArgs,
    /// MFT record number.
    #[arg(long)]
    record: u64,
    /// Named data stream (default: the unnamed stream).
    #[arg(long)]
    stream: Option<String>,
    /// Output file path.
    #[arg(long, short = 'o')]
    output: PathBuf,
}

pub fn run(cmd: Command) -> anyhow::Result<()> {
    match cmd {
        Command::Info(a) => info(a),
        Command::Ls(a) => ls(a),
        Command::Record(a) => record(a),
        Command::Extract(a) => extract(a),
    }
}

#[derive(Serialize)]
struct InfoReport<'a> {
    partition: Option<u32>,
    volume_offset: u64,
    boot: &'a phoinix_fs_ntfs::NtfsBootSector,
    volume: phoinix_fs_ntfs::VolumeInformation,
    mft_records: u64,
    mft_extents: usize,
    mft_from_mirror: bool,
}

fn info(args: InfoArgs) -> anyhow::Result<()> {
    let (selected, volume) = args.source.open()?;
    let vi = volume.volume_information().unwrap_or_default();
    let report = InfoReport {
        partition: selected.partition,
        volume_offset: selected.offset,
        boot: volume.boot(),
        volume: vi,
        mft_records: volume.mft().record_count(),
        mft_extents: volume.mft().stream().runs().len(),
        mft_from_mirror: volume.mft().used_mirror,
    };
    if args.json {
        return output::print_json(&report);
    }
    let b = report.boot;
    outln!("NTFS volume");
    if let Some(p) = report.partition {
        outln!(
            "Partition:         {p} (offset {} bytes)",
            grouped(report.volume_offset)
        );
    }
    outln!(
        "Label:             {}",
        report.volume.name.as_deref().unwrap_or("-")
    );
    if let Some((maj, min)) = report.volume.version {
        outln!("Version:           {maj}.{min}");
    }
    outln!("Sector size:       {}", b.bytes_per_sector);
    outln!("Cluster size:      {}", b.cluster_size);
    outln!("Total sectors:     {}", grouped(b.total_sectors));
    outln!("Total clusters:    {}", grouped(b.total_clusters()));
    outln!(
        "Volume size:       {}",
        bytes_iec(b.volume_bytes().unwrap_or(0))
    );
    outln!("MFT record size:   {}", b.mft_record_size);
    outln!("Index record size: {}", b.index_record_size);
    outln!("MFT LCN:           {}", b.mft_lcn);
    outln!("MFTMirr LCN:       {}", b.mft_mirror_lcn);
    outln!("MFT records:       {}", grouped(report.mft_records));
    outln!("MFT extents:       {}", report.mft_extents);
    if report.mft_from_mirror {
        outln!("Note:              $MFT record 0 was recovered from $MFTMirr");
    }
    outln!("Serial:            {:016X}", b.volume_serial);
    Ok(())
}

#[derive(Serialize)]
struct LsEntry {
    record: u64,
    sequence: u16,
    in_use: bool,
    directory: bool,
    name: Option<String>,
    path: String,
    path_uncertain: bool,
    size: Option<u64>,
    modified: Option<String>,
    streams: usize,
    diagnostics: Vec<String>,
}

fn is_system_record(file: &NtfsFile) -> bool {
    file.reference.record < 24 || file.name().is_some_and(|n| n.starts_with('$'))
}

fn ls(args: LsArgs) -> anyhow::Result<()> {
    let (_selected, volume) = args.source.open()?;
    let resolver = volume.resolver();
    let mut entries = Vec::new();
    for (number, result) in volume.files() {
        let file = match result {
            Ok(f) => f,
            Err(e) => {
                tracing::debug!(record = number, error = %e, "skipping record");
                continue;
            }
        };
        if !args.all && !file.in_use {
            continue;
        }
        if !args.system && is_system_record(&file) {
            continue;
        }
        if file.names.is_empty() && file.streams.is_empty() {
            continue;
        }
        let resolved = resolver.resolve(&file);
        entries.push(LsEntry {
            record: number,
            sequence: file.reference.sequence,
            in_use: file.in_use,
            directory: file.directory,
            name: file.name().map(str::to_owned),
            path: resolved.path,
            path_uncertain: resolved.uncertain,
            size: file.size(),
            modified: file
                .standard_information
                .as_ref()
                .map(|si| si.modified.to_iso8601()),
            streams: file.streams.len(),
            diagnostics: file.diagnostics.iter().map(ToString::to_string).collect(),
        });
    }
    if args.json {
        return output::print_json(&entries);
    }
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|e| {
            vec![
                e.record.to_string(),
                match (e.in_use, e.directory) {
                    (true, true) => "dir".into(),
                    (true, false) => "file".into(),
                    (false, true) => "dir (deleted)".into(),
                    (false, false) => "file (deleted)".into(),
                },
                e.size.map_or_else(|| "-".to_owned(), grouped),
                e.modified.clone().unwrap_or_else(|| "-".into()),
                e.path.clone(),
            ]
        })
        .collect();
    output::write_raw(&output::table(
        &["RECORD", "TYPE", "SIZE", "MODIFIED", "PATH"],
        &rows,
    ));
    Ok(())
}

#[derive(Serialize)]
struct RecordReport {
    header: phoinix_fs_ntfs::FileRecordHeader,
    attributes: Vec<AttributeReport>,
    file: NtfsFile,
    path: phoinix_fs_ntfs::ResolvedPath,
}

#[derive(Serialize)]
struct AttributeReport {
    offset: usize,
    header: phoinix_fs_ntfs::attribute::AttributeHeader,
    resident_length: Option<usize>,
    non_resident: Option<phoinix_fs_ntfs::attribute::NonResidentHeader>,
}

fn record(args: RecordArgs) -> anyhow::Result<()> {
    let (_selected, volume) = args.source.open()?;
    let raw = volume
        .mft()
        .raw_record(args.record)
        .context("reading record")?;
    let rec = volume.mft().record(args.record).context("parsing record")?;
    let file = volume.file(args.record)?;
    let path = volume.resolver().resolve(&file);
    let mut attributes = Vec::new();
    for attr in rec.attributes() {
        match attr {
            Ok(a) => attributes.push(AttributeReport {
                offset: a.offset,
                resident_length: a.resident_value().map(<[u8]>::len),
                non_resident: a.non_resident().cloned(),
                header: a.header,
            }),
            Err(e) => {
                eprintln!("warning: {e}");
                break;
            }
        }
    }
    let report = RecordReport {
        header: rec.header().clone(),
        attributes,
        file,
        path,
    };
    if args.json {
        return output::print_json(&report);
    }
    let h = &report.header;
    outln!("MFT record {}", args.record);
    outln!("  Sequence:      {}", h.sequence_number);
    outln!("  In use:        {}", h.in_use());
    outln!("  Directory:     {}", h.is_directory());
    outln!(
        "  Base record:   {}",
        if h.is_base() {
            "yes".to_owned()
        } else {
            h.base_reference.to_string()
        }
    );
    outln!("  Hard links:    {}", h.hard_link_count);
    outln!("  Used/alloc:    {}/{}", h.used_size, h.allocated_size);
    outln!("  LSN:           {}", h.log_sequence_number);
    outln!(
        "  Path:          {}{}",
        report.path.path,
        if report.path.uncertain {
            "  (uncertain)"
        } else {
            ""
        }
    );
    outln!();
    outln!("Attributes");
    for a in &report.attributes {
        let name = a
            .header
            .name
            .as_deref()
            .map_or(String::new(), |n| format!(" \"{n}\""));
        let mut flags = Vec::new();
        if a.header.is_compressed() {
            flags.push("compressed");
        }
        if a.header.is_encrypted() {
            flags.push("encrypted");
        }
        if a.header.is_sparse() {
            flags.push("sparse");
        }
        let flags = if flags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", flags.join(", "))
        };
        match (&a.non_resident, a.resident_length) {
            (Some(nr), _) => outln!(
                "  {:#06x}  {}{name}  non-resident  VCN {}..={}  real {}  init {}  alloc {}{flags}",
                a.offset,
                a.header.attribute_type,
                nr.starting_vcn,
                nr.last_vcn,
                grouped(nr.real_size),
                grouped(nr.initialized_size),
                grouped(nr.allocated_size)
            ),
            (None, Some(len)) => outln!(
                "  {:#06x}  {}{name}  resident  {len} bytes{flags}",
                a.offset,
                a.header.attribute_type
            ),
            (None, None) => outln!("  {:#06x}  {}{name}", a.offset, a.header.attribute_type),
        }
    }
    outln!();
    outln!("Names");
    for n in &report.file.names {
        outln!(
            "  {:?}  {:?}  parent {}  size {}",
            n.name,
            n.namespace,
            n.parent,
            grouped(n.real_size)
        );
    }
    if let Some(si) = &report.file.standard_information {
        outln!();
        outln!("Standard information");
        outln!("  Created:   {}", si.created);
        outln!("  Modified:  {}", si.modified);
        outln!("  Accessed:  {}", si.accessed);
        outln!("  Attributes: {:#010x}", si.file_attributes);
    }
    outln!();
    outln!("Data streams");
    for s in &report.file.streams {
        let label = s
            .name
            .as_deref()
            .map_or_else(|| "(unnamed)".to_owned(), |n| format!("\"{n}\""));
        match &s.storage {
            DataStorage::Resident { .. } => {
                outln!("  {label}  resident  {} bytes", grouped(s.logical_size))
            }
            DataStorage::NonResident { runs, complete, .. } => {
                outln!(
                    "  {label}  non-resident  {} bytes  {} extents{}",
                    grouped(s.logical_size),
                    s.extent_count(),
                    if *complete { "" } else { "  INCOMPLETE" }
                );
                for r in runs {
                    match r {
                        phoinix_fs_ntfs::NtfsRun::Data { vcn, lcn, clusters } => {
                            outln!("      VCN {vcn} → LCN {lcn}  ({clusters} clusters)");
                        }
                        phoinix_fs_ntfs::NtfsRun::Sparse { vcn, clusters } => {
                            outln!("      VCN {vcn} → sparse  ({clusters} clusters)")
                        }
                    }
                }
            }
            DataStorage::UnsupportedCompressed { .. } => outln!(
                "  {label}  compressed  {} bytes  (unsupported)",
                grouped(s.logical_size)
            ),
            DataStorage::UnsupportedEncrypted { .. } => outln!(
                "  {label}  encrypted  {} bytes  (unsupported)",
                grouped(s.logical_size)
            ),
        }
    }
    if !report.file.diagnostics.is_empty() {
        outln!();
        outln!("Diagnostics");
        for d in &report.file.diagnostics {
            outln!("  ⚠ {d}");
        }
    }
    if args.hex {
        outln!();
        output::write_raw(&hex_dump(0, &raw));
    }
    Ok(())
}

fn extract(args: ExtractArgs) -> anyhow::Result<()> {
    let (_selected, volume) = args.source.open()?;
    let file = volume.file(args.record)?;
    let stream = volume
        .open_stream(&file, args.stream.as_deref())
        .context("opening stream")?;
    let mut out = std::fs::File::create(&args.output)
        .with_context(|| format!("creating {}", args.output.display()))?;
    let mut cursor = stream.cursor();
    let written = std::io::copy(&mut cursor, &mut out).context("copying stream")?;
    out.flush()?;
    eprintln!(
        "wrote {} bytes to {}",
        grouped(written),
        args.output.display()
    );
    Ok(())
}

#[allow(dead_code)]
fn _assert_reader_arc(_: Arc<dyn phoinix_block::BlockReader>) {}
