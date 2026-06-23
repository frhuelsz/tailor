//! The `tailor` binary entry point: parse args, initialize logging, dispatch, map to an exit code.

mod cli;
mod error;
mod run;
mod scaffold;

use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

use crate::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose, cli.quiet);

    match run::dispatch(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
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
        .init();
}
