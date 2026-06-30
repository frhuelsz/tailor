//! The `tailor` binary entry point: parse args, initialize logging, dispatch, map to an exit code.

mod cli;
mod error;
mod run;
mod scaffold;

use std::process::ExitCode;
use std::time::{SystemTime, UNIX_EPOCH};

use clap::Parser;
use tracing_subscriber::{
    EnvFilter, fmt,
    fmt::{format::Writer, time::FormatTime},
};

use crate::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose, cli.quiet);

    match run::dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if run::use_color() {
                eprintln!("\u{1b}[1;31merror:\u{1b}[0m {error}");
            } else {
                eprintln!("error: {error}");
            }
            ExitCode::FAILURE
        }
    }
}

/// A compact wall-clock `HH:MM:SS` timer for the live `tracing` view (`meta/docs/logging.md` §5.6).
/// A full RFC3339 stamp dominates a scrolling build and the precise instant only matters post-hoc, so
/// the live formatter shows just the time-of-day (UTC, dependency-free); preserved on-disk IC logs keep
/// IC's own full-precision `time` field.
struct CompactTime;

impl FormatTime for CompactTime {
    fn format_time(&self, writer: &mut Writer<'_>) -> std::fmt::Result {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
            % 86_400;
        write!(
            writer,
            "{:02}:{:02}:{:02}",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60
        )
    }
}

/// Initialize tracing from `-v`/`-q` flags, overridable by `RUST_LOG`.
fn init_tracing(verbose: u8, quiet: u8) {
    let level = match i16::from(verbose) - i16::from(quiet) {
        i16::MIN..=-1 => "error",
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(run::use_color())
        .with_timer(CompactTime)
        .init();
}
