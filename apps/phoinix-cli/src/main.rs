//! PHOINIX command-line application.
//!
//! The CLI is a first-class front-end for the recovery engine. It contains no
//! recovery logic of its own: every command composes library crates.

#![forbid(unsafe_code)]
#![allow(clippy::print_stdout, clippy::print_stderr)]

use clap::{Parser, Subcommand};

/// PHOINIX — open-source, evidence-driven data recovery.
#[derive(Debug, Parser)]
#[command(name = "phoinix", version, about, long_about = None)]
struct Cli {
    /// Increase log verbosity (-v: debug, -vv: trace). Filenames may appear at
    /// debug level and above; recovered content is never logged.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print build information and exit.
    Version,
}

fn init_tracing(verbosity: u8) {
    let level = match verbosity {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("phoinix={level}")));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);
    match cli.command {
        Command::Version => {
            println!("phoinix {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
