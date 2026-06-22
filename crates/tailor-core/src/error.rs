//! Error types for `tailor-core`, including the typed errors the **ports** expose to adapters.
//!
//! Port error types (`ExecError`, `ResolveError`) live here, not in the adapter crates, so that the
//! port traits in `ports` can name them without `tailor-core` depending on its own adapters
//! (`meta/docs/architecture.md` §6). Adapters map their internal failures into these.

use std::path::PathBuf;

/// Errors from container execution adapters (the `Executor` / `ContainerRuntime` ports).
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("container runtime error: {0}")]
    Runtime(String),

    #[error("Image Customizer exited with code {code}")]
    IcFailed { code: i64, logs: String },

    #[error("execution cancelled")]
    Cancelled,

    #[error("{context}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Other(String),
}

/// Errors from base-image / digest resolution (the `BaseResolver` port).
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("failed to read local base `{}`: {source}", .path.display())]
    LocalRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("registry resolution failed for `{reference}`: {detail}")]
    Registry { reference: String, detail: String },

    #[error("{0}")]
    Other(String),
}

/// Top-level orchestration errors.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Config(#[from] tailor_config::ConfigError),

    #[error(transparent)]
    Resolve(#[from] ResolveError),

    #[error(transparent)]
    Exec(#[from] ExecError),

    #[error("tailor.lock is missing a `{platform}` entry for `{reference}`; run `tailor lock`")]
    LockMissing { reference: String, platform: String },

    #[error("tailor.lock is out of date: {detail}")]
    LockDrift { detail: String },

    #[error("image `{image}` declares no base for architecture `{arch}`")]
    MissingArchBase { image: String, arch: String },

    #[error("unknown toolchain id `{id}` (image `{image}`)")]
    UnknownToolchain { id: String, image: String },

    #[error("invalid `--select` entry `{entry}` (expected `axis=value`)")]
    SelectorSyntax { entry: String },

    #[error(
        "`--select` references axis `{axis}`, which image `{image}` does not declare (axes: {declared})"
    )]
    UnknownSelectorAxis {
        axis: String,
        image: String,
        declared: String,
    },

    #[error("no cells match the selection for image `{image}`")]
    NoCellsSelected { image: String },

    #[error("failed to access `{}`: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to (de)serialize `{}`: {source}", .path.display())]
    Serde {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
}
