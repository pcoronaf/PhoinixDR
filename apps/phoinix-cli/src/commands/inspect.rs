//! `phoinix inspect` — identify partition table and filesystems.

use std::path::PathBuf;

use phoinix_block::{BlockReader, SourceFingerprint};
use phoinix_core::fmt::{bytes_si, grouped};
use phoinix_fs::Detection;
use phoinix_image::ContainerInfo;
use phoinix_volume::{PartitionScheme, PartitionTable};
use serde::Serialize;

use crate::output::{self, outln};
use crate::source::{self, standard_probes};

/// Arguments for `phoinix inspect`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Device path or image file.
    source: PathBuf,
    /// Emit JSON instead of text.
    #[arg(long)]
    json: bool,
    /// Also compute the source fingerprint (reads the first and last MiB).
    #[arg(long)]
    fingerprint: bool,
}

#[derive(Serialize)]
struct Report {
    source: SourceReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    container: Option<ContainerInfo>,
    partition_table: PartitionTable,
    volumes: Vec<VolumeReport>,
}

#[derive(Serialize)]
struct SourceReport {
    path: String,
    size: u64,
    logical_sector_size: u32,
    physical_sector_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fingerprint: Option<SourceFingerprint>,
}

#[derive(Serialize)]
struct VolumeReport {
    /// Partition index, or `None` when the whole source is one volume.
    partition: Option<u32>,
    offset: u64,
    length: u64,
    detection: Detection,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let opened = source::open(&args.source)?;
    let reader = &opened.reader;
    let table = &opened.table;
    let probes = standard_probes();

    let mut volumes = Vec::new();
    if table.scheme == PartitionScheme::None && table.partitions.is_empty() {
        volumes.push(VolumeReport {
            partition: None,
            offset: 0,
            length: reader.len(),
            detection: probes.detect(&**reader),
        });
    } else {
        for part in table.volumes() {
            let Ok(view) = part.open(reader.clone()) else {
                continue;
            };
            volumes.push(VolumeReport {
                partition: Some(part.index),
                offset: part.start_offset,
                length: view.len(),
                detection: probes.detect(&view),
            });
        }
    }

    let fingerprint = if args.fingerprint {
        Some(SourceFingerprint::compute(&**reader)?)
    } else {
        None
    };
    let report = Report {
        source: SourceReport {
            path: args.source.display().to_string(),
            size: reader.len(),
            logical_sector_size: reader.geometry().logical_sector_size,
            physical_sector_size: reader.geometry().physical_sector_size,
            fingerprint,
        },
        container: opened.container.clone().filter(ContainerInfo::is_container),
        partition_table: table.clone(),
        volumes,
    };

    if args.json {
        return output::print_json(&report);
    }
    print_text(&report);
    Ok(())
}

fn printed_container(index: u32) -> bool {
    use std::cell::RefCell;
    thread_local! {
        static PRINTED: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    }
    PRINTED.with(|p| {
        let mut p = p.borrow_mut();
        if p.contains(&index) {
            true
        } else {
            p.push(index);
            false
        }
    })
}

fn print_text(report: &Report) {
    let s = &report.source;
    outln!("Source");
    outln!("  Path:         {}", s.path);
    outln!(
        "  Size:         {} bytes ({})",
        grouped(s.size),
        bytes_si(s.size)
    );
    match s.physical_sector_size {
        Some(p) if p != s.logical_sector_size => outln!(
            "  Sector size:  {} logical / {p} physical",
            s.logical_sector_size
        ),
        _ => outln!("  Sector size:  {}", s.logical_sector_size),
    }
    if let Some(fp) = &s.fingerprint {
        outln!(
            "  Fingerprint:  {}",
            phoinix_block::to_hex(&fp.first_mib_sha256)
        );
    }

    if let Some(c) = &report.container {
        outln!();
        print_container(c);
    }

    let t = &report.partition_table;
    outln!();
    outln!("Partition table");
    outln!("  Scheme:       {}", t.scheme);
    if let Some(guid) = t.disk_guid {
        outln!(
            "  Disk GUID:    {}",
            guid.hyphenated().to_string().to_uppercase()
        );
    }
    if let Some(sig) = t.mbr_disk_signature
        && t.scheme == PartitionScheme::Mbr
    {
        outln!("  Disk signature: {sig:#010x}");
    }
    if t.diagnostics.is_empty() {
        if t.scheme == PartitionScheme::Gpt {
            outln!("  Header:       valid");
            outln!("  Backup:       valid");
        }
    } else {
        outln!("  Diagnostics:");
        for d in &t.diagnostics {
            outln!("    - {d}");
        }
    }

    outln!();
    let whole_source = report.volumes.len() == 1
        && report
            .volumes
            .first()
            .is_some_and(|v| v.partition.is_none());
    outln!(
        "{}",
        if whole_source {
            "Volume (whole source)"
        } else {
            "Partitions"
        }
    );
    let containers: Vec<&phoinix_volume::Partition> = t
        .partitions
        .iter()
        .filter(|p| p.partition_type.is_extended_container())
        .collect();
    for v in &report.volumes {
        let part = v
            .partition
            .and_then(|i| t.partitions.iter().find(|p| p.index == i));
        // List an extended container just before its first logical partition.
        if let Some(p) = part
            && p.flags.contains(phoinix_volume::PartitionFlags::LOGICAL)
        {
            for c in &containers {
                if c.start_lba <= p.start_lba
                    && p.end_lba <= c.end_lba
                    && !printed_container(c.index)
                {
                    outln!(
                        "{}  {}  [container]",
                        c.index,
                        c.partition_type.description()
                    );
                    outln!(
                        "   Sectors:  {} – {}",
                        grouped(c.start_lba),
                        grouped(c.end_lba)
                    );
                    outln!();
                }
            }
        }
        let title = match (v.partition, part) {
            (Some(i), Some(p)) => {
                let name = p
                    .name
                    .clone()
                    .unwrap_or_else(|| p.partition_type.description());
                let mut flags = Vec::new();
                if p.flags.contains(phoinix_volume::PartitionFlags::BOOTABLE) {
                    flags.push("bootable");
                }
                if p.flags.contains(phoinix_volume::PartitionFlags::LOGICAL) {
                    flags.push("logical");
                }
                if p.confidence != phoinix_volume::PartitionConfidence::High {
                    flags.push("suspect");
                }
                let flags = if flags.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", flags.join(", "))
                };
                format!("{i}  {name}{flags}")
            }
            _ => String::new(),
        };
        if !title.is_empty() {
            outln!("{title}");
        }
        if let Some(p) = part {
            outln!("   Type:     {}", p.partition_type.description());
            outln!(
                "   Sectors:  {} – {}",
                grouped(p.start_lba),
                grouped(p.end_lba)
            );
        }
        outln!("   Offset:   {} bytes", grouped(v.offset));
        outln!(
            "   Size:     {} ({} bytes)",
            bytes_si(v.length),
            grouped(v.length)
        );
        match &v.detection.best {
            Some(best) => {
                outln!(
                    "   FS:       {} (confidence {}%)",
                    best.filesystem,
                    best.confidence
                );
                for e in &best.evidence {
                    outln!(
                        "             {} {}",
                        if e.supports { "✓" } else { "⚠" },
                        e.description
                    );
                }
            }
            None => outln!("   FS:       unknown"),
        }
        outln!();
    }
}

/// Prints the container section shared by `inspect` and `verify`.
pub fn print_container(c: &ContainerInfo) {
    outln!("Image container");
    outln!("  Format:       {} ({})", c.format, c.variant);
    if c.segments.len() > 1 {
        outln!(
            "  Segments:     {} files, {} … {}",
            c.segments.len(),
            c.segments
                .first()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            c.segments
                .last()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        );
    }
    outln!(
        "  Media size:   {} bytes ({}), {}-byte sectors",
        grouped(c.size),
        bytes_si(c.size),
        c.sector_size
    );
    if let Some(u) = c.unit_size {
        outln!(
            "  Unit:         {} bytes per chunk/block",
            grouped(u64::from(u))
        );
    }
    if let Some(m) = &c.compression {
        outln!("  Compression:  {m}");
    }
    if let Some(m) = &c.media_type {
        outln!("  Media type:   {m}");
    }
    if let Some(id) = &c.identifier {
        outln!("  Identifier:   {id}");
    }
    if let Some(md5) = &c.stored_hashes.md5 {
        outln!("  Stored MD5:   {md5}");
    }
    if let Some(sha1) = &c.stored_hashes.sha1 {
        outln!("  Stored SHA-1: {sha1}");
    }
    if let Some(n) = c.acquisition_errors {
        outln!("  Read errors:  {n} sector ranges the imaging tool could not read");
    }
    if let Some(a) = &c.acquisition {
        outln!("  Acquisition:");
        let rows: [(&str, &Option<String>); 11] = [
            ("case number", &a.case_number),
            ("evidence number", &a.evidence_number),
            ("description", &a.description),
            ("examiner", &a.examiner),
            ("notes", &a.notes),
            ("acquired", &a.acquisition_date),
            ("system date", &a.system_date),
            ("software", &a.software_version),
            ("operating system", &a.operating_system),
            ("model", &a.model),
            ("serial number", &a.serial_number),
        ];
        for (k, v) in rows {
            if let Some(v) = v {
                outln!("    {k:<17} {v}");
            }
        }
    }
    for d in &c.diagnostics {
        outln!("  ⚠ {d}");
    }
}
