//! `phoinix devices`

use phoinix_core::fmt::bytes_si;
use phoinix_device::{DeviceKind, platform_enumerator};

use crate::output;

/// Arguments for `phoinix devices`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
    /// Include partition nodes as well as whole disks.
    #[arg(long)]
    partitions: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    let enumerator = platform_enumerator();
    let mut devices = enumerator.enumerate()?;
    if !args.partitions {
        devices.retain(|d| d.kind == DeviceKind::Disk);
    }
    if args.json {
        return output::print_json(&devices);
    }
    if devices.is_empty() {
        println!("No block devices found (or none are accessible to this process).");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = devices
        .iter()
        .map(|d| {
            let size = if d.accessible || d.size > 0 {
                bytes_si(d.size)
            } else {
                "access denied".to_owned()
            };
            let sector = match d.geometry.physical_sector_size {
                Some(p) if p != d.geometry.logical_sector_size => {
                    format!("{}/{}", d.geometry.logical_sector_size, p)
                }
                _ => d.geometry.logical_sector_size.to_string(),
            };
            let media = match d.rotational {
                Some(true) => "HDD",
                Some(false) => "SSD",
                None => "-",
            };
            vec![
                d.path.to_string(),
                output::opt(d.bus),
                size,
                sector,
                media.to_owned(),
                d.display_name.clone(),
                output::opt(d.serial.as_deref()),
            ]
        })
        .collect();
    print!(
        "{}",
        output::table(
            &[
                "DEVICE", "BUS", "SIZE", "SECTOR", "MEDIA", "MODEL", "SERIAL"
            ],
            &rows
        )
    );
    if devices.iter().any(|d| !d.accessible) {
        eprintln!("\nSome devices could not be opened; run with elevated privileges to read them.");
    }
    Ok(())
}
