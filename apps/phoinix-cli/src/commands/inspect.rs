//! `phoinix inspect` — identify partition table and filesystems.

use std::path::PathBuf;

use phoinix_block::{BlockReader, SourceFingerprint};
use phoinix_core::fmt::{bytes_si, grouped};
use phoinix_fs::Detection;
use phoinix_volume::{PartitionScheme, PartitionTable};
use serde::Serialize;

use crate::output;
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
    println!("Source");
    println!("  Path:         {}", s.path);
    println!(
        "  Size:         {} bytes ({})",
        grouped(s.size),
        bytes_si(s.size)
    );
    match s.physical_sector_size {
        Some(p) if p != s.logical_sector_size => println!(
            "  Sector size:  {} logical / {p} physical",
            s.logical_sector_size
        ),
        _ => println!("  Sector size:  {}", s.logical_sector_size),
    }
    if let Some(fp) = &s.fingerprint {
        println!(
            "  Fingerprint:  {}",
            phoinix_block::to_hex(&fp.first_mib_sha256)
        );
    }

    let t = &report.partition_table;
    println!();
    println!("Partition table");
    println!("  Scheme:       {}", t.scheme);
    if let Some(guid) = t.disk_guid {
        println!(
            "  Disk GUID:    {}",
            guid.hyphenated().to_string().to_uppercase()
        );
    }
    if let Some(sig) = t.mbr_disk_signature
        && t.scheme == PartitionScheme::Mbr
    {
        println!("  Disk signature: {sig:#010x}");
    }
    if t.diagnostics.is_empty() {
        if t.scheme == PartitionScheme::Gpt {
            println!("  Header:       valid");
            println!("  Backup:       valid");
        }
    } else {
        println!("  Diagnostics:");
        for d in &t.diagnostics {
            println!("    - {d}");
        }
    }

    println!();
    let whole_source = report.volumes.len() == 1
        && report
            .volumes
            .first()
            .is_some_and(|v| v.partition.is_none());
    println!(
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
                    println!(
                        "{}  {}  [container]",
                        c.index,
                        c.partition_type.description()
                    );
                    println!(
                        "   Sectors:  {} – {}",
                        grouped(c.start_lba),
                        grouped(c.end_lba)
                    );
                    println!();
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
            println!("{title}");
        }
        if let Some(p) = part {
            println!("   Type:     {}", p.partition_type.description());
            println!(
                "   Sectors:  {} – {}",
                grouped(p.start_lba),
                grouped(p.end_lba)
            );
        }
        println!("   Offset:   {} bytes", grouped(v.offset));
        println!(
            "   Size:     {} ({} bytes)",
            bytes_si(v.length),
            grouped(v.length)
        );
        match &v.detection.best {
            Some(best) => {
                println!(
                    "   FS:       {} (confidence {}%)",
                    best.filesystem, best.confidence
                );
                for e in &best.evidence {
                    println!(
                        "             {} {}",
                        if e.supports { "✓" } else { "⚠" },
                        e.description
                    );
                }
            }
            None => println!("   FS:       unknown"),
        }
        println!();
    }
}
