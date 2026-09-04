//! PHOINIX command-line application.
//!
//! The CLI is a first-class front-end for the recovery engine. It contains no
//! recovery logic of its own: every command composes library crates.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

mod commands;
mod output;
mod source;

use clap::{Parser, Subcommand};

/// PHOINIX — open-source, evidence-driven data recovery.
#[derive(Debug, Parser)]
#[command(name = "phoinix", version, about, long_about = None)]
struct Cli {
    /// Increase log verbosity (-v: info, -vv: debug, -vvv: trace). Filenames
    /// may appear at debug level and above; recovered content is never logged.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List block devices visible to this process.
    Devices(commands::devices::Args),
    /// Read raw bytes from a source (developer/debug command).
    Read(commands::read::Args),
    /// Identify the partition table and filesystems of a device or image.
    Inspect(commands::inspect::Args),
    /// Native NTFS reader commands.
    #[command(subcommand)]
    Ntfs(commands::ntfs::Command),
    /// Scan a source for recoverable files and assess their health.
    Scan(commands::scan::Args),
    /// Explain the evidence behind a candidate's recovery health.
    Explain(commands::explain::Args),
    /// Recover candidates to another filesystem and verify them.
    Recover(commands::recover::Args),
}

fn init_tracing(verbosity: u8) {
    let level = match verbosity {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("phoinix={level}")));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Devices(args) => commands::devices::run(args),
        Command::Read(args) => commands::read::run(args),
        Command::Inspect(args) => commands::inspect::run(args),
        Command::Ntfs(cmd) => commands::ntfs::run(cmd),
        Command::Scan(args) => commands::scan::run(args),
        Command::Explain(args) => commands::explain::run(args),
        Command::Recover(args) => commands::recover::run(args),
    }
}

fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
            || cause
                .downcast_ref::<serde_json::Error>()
                .is_some_and(|e| e.io_error_kind() == Some(std::io::ErrorKind::BrokenPipe))
    })
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            // A consumer closing the pipe early (e.g. `| head`) is not an error.
            if is_broken_pipe(&err) {
                return std::process::ExitCode::SUCCESS;
            }
            eprintln!("error: {err:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
