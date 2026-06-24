//! Application error type — wraps the typed library errors and adds CLI-level context.

/// Errors surfaced by the `tailor` binary.
#[derive(Debug, thiserror::Error)]
pub(crate) enum AppError {
    #[error(transparent)]
    Core(#[from] tailor_core::CoreError),

    #[error(transparent)]
    Config(#[from] tailor_config::ConfigError),

    #[error(transparent)]
    Resolve(#[from] tailor_core::ResolveError),

    #[error(transparent)]
    Exec(#[from] tailor_core::ExecError),

    #[error(transparent)]
    Sign(#[from] tailor_core::SignError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Message(String),
}
