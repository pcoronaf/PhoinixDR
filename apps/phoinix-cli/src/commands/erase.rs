//! `phoinix erase`: secure erase of a device, partition, file, the free
//! space of a volume or a mounted volume (milestone M12). The only command
//! that writes to a source.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use phoinix_core::fmt::bytes_si;
use phoinix_erase::{
    ARTICLE_GUTMANN_MYTH, ERASE_WARNING, EraseEvent, EraseMethod, EraseRequest, EraseTarget,
    FLASH_WARNING, MULTI_PASS_WARNING, Operator, Pass, PassPattern, PostScan, VerifyMode,
};
use phoinix_session::{SessionScanner, Workspace};

use crate::output::{self, outln};

/// `phoinix erase` subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// List the disks that can be erased, with the reasons the others cannot.
    List,
    /// Describe a target without writing anything.
    Describe(TargetArgs),
    /// Erase a whole device, partition table included.
    Device(EraseArgs),
    /// Erase one partition of a device (the table entry stays).
    Partition(PartitionArgs),
    /// Erase a regular file (a disk image before disposal) as a raw range.
    File(EraseArgs),
    /// Overwrite the free space of a volume (NTFS, FAT, exFAT, ext) on a
    /// device or image, found through the read-only engines; the volume
    /// must not be mounted.
    FreeSpace(FreeSpaceArgs),
    /// Fill the free space of a mounted volume through the filesystem (no
    /// administrator rights needed; the volume stays in use).
    Mounted(MountedArgs),
}

/// A target path.
#[derive(Debug, clap::Args)]
pub struct TargetArgs {
    /// Device path (`\\.\PhysicalDrive2`, `/dev/sdb`) or file.
    pub path: PathBuf,
    /// Partition index (1-based), to describe a partition instead of the device.
    #[arg(long, short = 'p')]
    pub partition: Option<u32>,
    /// Describe the free space of the volume (with `--partition` when the
    /// source has several) instead of the raw range.
    #[arg(long)]
    pub free_space: bool,
    /// With `--free-space`: include the slack of the allocated files.
    #[arg(long)]
    pub slack: bool,
}

/// Free-space erase options.
#[derive(Debug, clap::Args)]
pub struct FreeSpaceArgs {
    /// Device or image path.
    pub path: PathBuf,
    /// Partition index (1-based); needed when the source has several.
    #[arg(long, short = 'p')]
    pub partition: Option<u32>,
    /// Also overwrite the slack at the end of every allocated file.
    #[arg(long)]
    pub slack: bool,
    #[command(flatten)]
    pub options: Options,
}

/// Mounted-volume erase options.
#[derive(Debug, clap::Args)]
pub struct MountedArgs {
    /// The drive (`E:\\`), the mount point, or any folder on the volume (the
    /// fill files are created there).
    pub path: PathBuf,
    /// Bytes to leave free (accepts `512M`, `2G`); 0 fills the volume.
    #[arg(long, default_value = "0")]
    pub reserve: String,
    #[command(flatten)]
    pub options: Options,
}

/// Erase options.
#[derive(Debug, clap::Args)]
pub struct EraseArgs {
    /// Device or file path.
    pub path: PathBuf,
    #[command(flatten)]
    pub options: Options,
}

/// Partition erase options.
#[derive(Debug, clap::Args)]
pub struct PartitionArgs {
    /// Device path.
    pub path: PathBuf,
    /// Partition index (1-based, table order).
    #[arg(long, short = 'p')]
    pub partition: u32,
    #[command(flatten)]
    pub options: Options,
}

/// Options shared by the erase commands.
#[derive(Debug, clap::Args)]
pub struct Options {
    /// Method: zero, random, nist, dod-3, dod-7, schneier, vsitr, gutmann, custom.
    #[arg(long, default_value = "zero")]
    pub method: String,
    /// For `--method custom`: number of passes (random data).
    #[arg(long, default_value_t = 1)]
    pub passes: usize,
    /// For `--method custom`: a hex byte pattern to repeat instead of random data.
    #[arg(long)]
    pub pattern: Option<String>,
    /// Read-back verification: none, sample, full.
    #[arg(long, default_value = "sample")]
    pub verify: String,
    /// Scan the target with the recovery engines afterwards and record the
    /// outcome in the certificate: none, quick, deep.
    #[arg(long, default_value = "none")]
    pub scan: String,
    /// Write the sanitization certificate here (`.json`, `.md`, `.html`).
    #[arg(long, value_name = "PATH")]
    pub certificate: Option<PathBuf>,
    /// Operator name for the certificate.
    #[arg(long)]
    pub operator: Option<String>,
    /// Organisation for the certificate.
    #[arg(long)]
    pub organisation: Option<String>,
    /// Asset, case or ticket number for the certificate.
    #[arg(long)]
    pub asset: Option<String>,
    /// Notes for the certificate.
    #[arg(long)]
    pub notes: Option<String>,
    /// Confirm non-interactively by passing the device's serial number (or
    /// its last six characters; the file name for a file). Without it the
    /// command asks on the terminal.
    #[arg(long, value_name = "SERIAL")]
    pub i_understand_this_destroys_data: Option<String>,
    /// Continue a cancelled erase of the same target from its resume point.
    #[arg(long)]
    pub resume: bool,
    /// Directory for resume points (default: the sessions directory).
    #[arg(long, value_name = "DIR")]
    pub state_dir: Option<PathBuf>,
    /// Allow a target on the disk that holds the running executable.
    #[arg(long)]
    pub allow_executable_disk: bool,
    /// Allow a target on the disk that holds the sessions directory.
    #[arg(long)]
    pub allow_sessions_disk: bool,
    /// Skip the five-second countdown.
    #[arg(long)]
    pub no_countdown: bool,
    /// Emit JSON events instead of text.
    #[arg(long)]
    pub json: bool,
}

fn sessions_dir() -> PathBuf {
    std::env::var_os("PHOINIX_SESSIONS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("phoinixdr-sessions"))
}

fn parse_method(options: &Options) -> anyhow::Result<EraseMethod> {
    if options.method.eq_ignore_ascii_case("custom") {
        let pattern = match &options.pattern {
            Some(hex_text) => {
                let bytes = hex::decode(hex_text.trim_start_matches("0x"))
                    .map_err(|e| anyhow::anyhow!("--pattern must be hex bytes: {e}"))?;
                if bytes.is_empty() {
                    anyhow::bail!("--pattern must not be empty");
                }
                PassPattern::Sequence { bytes }
            }
            None => PassPattern::Random,
        };
        if options.passes == 0 {
            anyhow::bail!("--passes must be at least 1");
        }
        return Ok(EraseMethod::Custom {
            passes: std::iter::repeat_n(Pass { pattern }, options.passes).collect(),
        });
    }
    EraseMethod::parse(&options.method).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown method {:?}; one of {}",
            options.method,
            EraseMethod::builtin()
                .iter()
                .map(EraseMethod::id)
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

fn parse_scan(text: &str) -> anyhow::Result<PostScan> {
    PostScan::parse(text)
        .ok_or_else(|| anyhow::anyhow!("unknown --scan {text:?}; one of none, quick, deep"))
}

/// Parses a byte count with an optional K/M/G/T suffix.
fn parse_bytes(text: &str) -> anyhow::Result<u64> {
    let t = text.trim().to_ascii_uppercase();
    let (digits, factor) = match t.chars().last() {
        Some('K') => (t.trim_end_matches('K'), 1u64 << 10),
        Some('M') => (t.trim_end_matches('M'), 1u64 << 20),
        Some('G') => (t.trim_end_matches('G'), 1u64 << 30),
        Some('T') => (t.trim_end_matches('T'), 1u64 << 40),
        _ => (t.as_str(), 1u64),
    };
    let n: u64 = digits
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("not a byte count: {text:?}"))?;
    n.checked_mul(factor)
        .ok_or_else(|| anyhow::anyhow!("byte count too large: {text:?}"))
}

fn parse_verify(text: &str) -> anyhow::Result<VerifyMode> {
    match text.to_ascii_lowercase().as_str() {
        "none" => Ok(VerifyMode::None),
        "sample" => Ok(VerifyMode::Sample),
        "full" => Ok(VerifyMode::Full),
        other => anyhow::bail!("unknown --verify {other:?}; one of none, sample, full"),
    }
}

/// Runs the subcommand.
///
/// # Errors
///
/// Returns an error for refused targets, bad confirmation or I/O failures.
pub fn run(command: Command) -> anyhow::Result<()> {
    let workspace = Workspace::new(sessions_dir());
    match command {
        Command::List => list(&workspace),
        Command::Describe(args) => {
            let target = if args.free_space {
                EraseTarget::FreeSpace {
                    path: args.path,
                    partition: args.partition,
                    slack: args.slack,
                }
            } else {
                match args.partition {
                    Some(index) => EraseTarget::Partition {
                        path: args.path,
                        index,
                    },
                    None => target_for(&args.path),
                }
            };
            let desc = workspace.describe_erase_target(&target)?;
            print_description(&desc);
            if !desc.refusals.is_empty() {
                anyhow::bail!("this target is refused");
            }
            Ok(())
        }
        Command::Device(args) => erase(&workspace, target_for(&args.path), args.options),
        Command::Partition(args) => erase(
            &workspace,
            EraseTarget::Partition {
                path: args.path,
                index: args.partition,
            },
            args.options,
        ),
        Command::File(args) => erase(
            &workspace,
            EraseTarget::File { path: args.path },
            args.options,
        ),
        Command::FreeSpace(args) => erase(
            &workspace,
            EraseTarget::FreeSpace {
                path: args.path,
                partition: args.partition,
                slack: args.slack,
            },
            args.options,
        ),
        Command::Mounted(args) => {
            let reserve = parse_bytes(&args.reserve)?;
            erase(
                &workspace,
                EraseTarget::MountedFreeSpace {
                    mount: args.path,
                    reserve,
                },
                args.options,
            )
        }
    }
}

fn target_for(path: &std::path::Path) -> EraseTarget {
    if path.is_file() {
        EraseTarget::File {
            path: path.to_path_buf(),
        }
    } else if path.is_dir() {
        EraseTarget::MountedFreeSpace {
            mount: path.to_path_buf(),
            reserve: 0,
        }
    } else {
        EraseTarget::WholeDevice {
            path: path.to_path_buf(),
        }
    }
}

fn list(workspace: &Workspace) -> anyhow::Result<()> {
    let targets = workspace.erase_targets()?;
    if targets.is_empty() {
        outln!("No disks found (device access may need administrator rights).");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = targets
        .iter()
        .map(|t| {
            let status = if let Some(e) = &t.error {
                format!("cannot read: {e}")
            } else if t.refusals.is_empty() {
                "may be erased".to_owned()
            } else {
                format!(
                    "refused: {}",
                    t.refusals
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            };
            vec![
                t.device.path.as_str().to_owned(),
                t.device.display_name.clone(),
                t.device.serial.clone().unwrap_or_default(),
                bytes_si(t.device.size),
                match t.device.rotational {
                    Some(false) => "SSD".into(),
                    Some(true) => "HDD".into(),
                    None => String::new(),
                },
                t.description
                    .as_ref()
                    .map(|d| {
                        d.volumes
                            .iter()
                            .map(|v| v.mount.clone())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default(),
                status,
            ]
        })
        .collect();
    outln!(
        "{}",
        output::table(
            &[
                "device", "model", "serial", "size", "medium", "mounted", "status"
            ],
            &rows
        )
    );
    let volumes = workspace.erase_volumes();
    if !volumes.is_empty() {
        outln!();
        outln!(
            "Mounted volumes (free space can be filled through the filesystem with `phoinix erase mounted`):"
        );
        let rows: Vec<Vec<String>> = volumes
            .iter()
            .map(|v| {
                vec![
                    v.mount.clone(),
                    v.filesystem.clone(),
                    v.label.clone().unwrap_or_default(),
                    bytes_si(v.total),
                    bytes_si(v.free),
                    v.disk.clone().unwrap_or_default(),
                    if v.read_only {
                        "read-only".into()
                    } else if v.system {
                        "system volume".into()
                    } else {
                        String::new()
                    },
                ]
            })
            .collect();
        outln!(
            "{}",
            output::table(
                &[
                    "mount",
                    "filesystem",
                    "label",
                    "size",
                    "free",
                    "disk",
                    "note"
                ],
                &rows
            )
        );
    }
    outln!("{ERASE_WARNING}");
    Ok(())
}

fn print_description(desc: &phoinix_erase::TargetDescription) {
    outln!("Target: {} {}", desc.target.kind(), desc.path);
    if let Some(d) = &desc.device {
        outln!(
            "Device: {} · S/N {} · {} · {}",
            d.display_name,
            d.serial.as_deref().unwrap_or("unknown"),
            bytes_si(d.size),
            match d.rotational {
                Some(false) => "solid state",
                Some(true) => "rotational",
                None => "medium unknown",
            }
        );
    }
    if let Some(sp) = &desc.space {
        if let Some(m) = &sp.mount {
            outln!(
                "Volume: {} at {} ({}{}) · {} free of {}",
                sp.filesystem,
                m,
                sp.node.as_deref().unwrap_or("?"),
                sp.label
                    .as_deref()
                    .map(|l| format!(", label {l:?}"))
                    .unwrap_or_default(),
                bytes_si(sp.free_bytes),
                bytes_si(sp.total_bytes.unwrap_or(sp.volume_length))
            );
        } else {
            outln!(
                "Volume: {} at offset {} · {} · cluster {} · free space {} in {} range(s){}",
                sp.filesystem,
                sp.volume_offset,
                bytes_si(sp.volume_length),
                bytes_si(sp.cluster_size),
                bytes_si(sp.free_bytes),
                sp.free_ranges,
                if sp.slack {
                    format!(
                        " · slack {} in {} range(s)",
                        bytes_si(sp.slack_bytes),
                        sp.slack_ranges
                    )
                } else {
                    String::new()
                }
            );
        }
        outln!("To write: {}", bytes_si(desc.length));
    } else {
        outln!("Range: offset {} · {}", desc.offset, bytes_si(desc.length));
    }
    if let Some(p) = &desc.partition {
        outln!("Partition {}: {}", p.index, p.type_name);
    }
    for p in &desc.partitions {
        outln!(
            "  partition {} at {} · {} · {}",
            p.index,
            p.offset,
            bytes_si(p.length),
            p.type_name
        );
    }
    for v in &desc.volumes {
        outln!(
            "  mounted: {} at {}{}",
            v.node,
            v.mount,
            if v.can_dismount {
                " (will be dismounted)"
            } else {
                " (unmount it first)"
            }
        );
    }
    for r in &desc.refusals {
        outln!("REFUSED: {r}");
    }
    for w in &desc.warnings {
        outln!("Note: {w}");
    }
    if desc.solid_state == Some(true) {
        outln!("{FLASH_WARNING}");
    }
}

fn erase(workspace: &Workspace, target: EraseTarget, options: Options) -> anyhow::Result<()> {
    let method = parse_method(&options)?;
    let verify = parse_verify(&options.verify)?;
    let post_scan = parse_scan(&options.scan)?;
    let desc = workspace.describe_erase_target(&target)?;
    let json = options.json;
    if !json {
        print_description(&desc);
        outln!("Method: {} — {}", method.label(), method.note());
        if method.multi_pass() {
            outln!("{MULTI_PASS_WARNING} Read: {ARTICLE_GUTMANN_MYTH}");
        }
        outln!("{ERASE_WARNING}");
    }
    if let Some(r) = desc.refusals.first() {
        anyhow::bail!("refused: {r}");
    }
    let confirmation = match options.i_understand_this_destroys_data.clone() {
        Some(text) => text,
        None => {
            if !std::io::stdin().is_terminal() {
                anyhow::bail!(
                    "no terminal to confirm on; pass --i-understand-this-destroys-data {:?}",
                    desc.confirmation_short
                        .clone()
                        .unwrap_or_else(|| desc.confirmation.clone())
                );
            }
            eprint!(
                "Type {:?}{} to erase, anything else to abort: ",
                desc.confirmation,
                desc.confirmation_short
                    .as_deref()
                    .map(|s| format!(" (or its last six characters {s:?})"))
                    .unwrap_or_default()
            );
            let _ = std::io::stderr().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim().to_owned()
        }
    };
    if !desc.confirms(&confirmation) {
        anyhow::bail!("confirmation does not match; nothing was written");
    }
    if !options.no_countdown && !json {
        for n in (1..=5).rev() {
            eprint!("\rErasing in {n} s… press Ctrl+C to abort ");
            let _ = std::io::stderr().flush();
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        eprintln!();
    }
    let request = EraseRequest {
        target,
        method,
        verify,
        operator: Operator {
            name: options.operator,
            organisation: options.organisation,
            asset: options.asset,
            notes: options.notes,
        },
        certificate: options.certificate,
        confirmation,
        resume: options.resume,
        state_dir: Some(
            options
                .state_dir
                .unwrap_or_else(|| workspace.sessions_dir().to_path_buf()),
        ),
        allow_executable_disk: options.allow_executable_disk,
        allow_sessions_disk: options.allow_sessions_disk,
        post_scan,
    };
    let progress_tty = std::io::stderr().is_terminal() && !json;
    let mut sink = |e: EraseEvent| {
        if json {
            if let Ok(line) = serde_json::to_string(&e) {
                outln!("{line}");
            }
            return;
        }
        match &e {
            EraseEvent::Started { passes, .. } => outln!("Erasing: {} pass(es)", passes),
            EraseEvent::Pass {
                pass,
                passes,
                pattern,
            } => {
                if progress_tty {
                    eprintln!();
                }
                outln!("Pass {pass} of {passes}: {pattern}");
            }
            EraseEvent::Progress {
                done,
                total,
                bytes_per_sec,
                unwritable,
                ..
            } if progress_tty => {
                let pct = if *total > 0 { done * 100 / total } else { 100 };
                eprint!(
                    "\r  {pct}% ({} of {}) at {}/s{}   ",
                    bytes_si(*done),
                    bytes_si(*total),
                    bytes_si(*bytes_per_sec),
                    if *unwritable > 0 {
                        format!(", {} unwritable", bytes_si(*unwritable))
                    } else {
                        String::new()
                    }
                );
                let _ = std::io::stderr().flush();
            }
            EraseEvent::Verifying { done, total } if progress_tty => {
                eprint!("\r  verifying {done} of {total} blocks   ");
                let _ = std::io::stderr().flush();
            }
            EraseEvent::Scanning {
                phase,
                done,
                total,
                candidates,
                ..
            } if progress_tty => {
                eprint!(
                    "\r  post-erase scan: {phase} {done}{} · {candidates} candidate(s)   ",
                    total.map(|t| format!(" of {t}")).unwrap_or_default()
                );
                let _ = std::io::stderr().flush();
            }
            EraseEvent::Finished { summary } => {
                if progress_tty {
                    eprintln!();
                }
                outln!("Done: {}", summary.verdict);
                if let Some(c) = &summary.certificate {
                    outln!("Certificate: {}", c.display());
                }
            }
            EraseEvent::Cancelled { resume, summary } => {
                if progress_tty {
                    eprintln!();
                }
                match resume {
                    Some(resume) => outln!(
                        "Cancelled: {}. Resume with --resume (pass {}, offset {}).",
                        summary.verdict,
                        resume.pass + 1,
                        resume.offset
                    ),
                    None => outln!(
                        "Cancelled: {}. The fill files were removed.",
                        summary.verdict
                    ),
                }
            }
            EraseEvent::Failed { message, summary } => {
                if progress_tty {
                    eprintln!();
                }
                outln!("Failed: {message}. {}", summary.verdict);
            }
            _ => {}
        }
    };
    let outcome = phoinix_erase::run_erase_with(
        &request,
        &workspace.erase_guards(),
        Some(&SessionScanner),
        &mut sink,
        &AtomicBool::new(false),
    )?;
    match outcome.result {
        Ok(()) => {
            let verified = outcome
                .summary
                .verification
                .as_ref()
                .is_none_or(|v| v.passed || v.mode == VerifyMode::None);
            let scan_clean = outcome
                .summary
                .scan
                .as_ref()
                .is_none_or(|s| !s.ran || s.passed);
            if outcome.summary.unwritable_bytes > 0 || !verified || !scan_clean {
                anyhow::bail!(
                    "the erase completed with regions not erased, not verified, or with recoverable data found by the post-erase scan; see the certificate"
                )
            }
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
