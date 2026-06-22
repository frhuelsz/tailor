//! Port traits — the hexagonal boundary `tailor-core` defines and adapters implement
//! (`meta/docs/architecture.md` §4.2). Traits use return-position `impl Future` (no `async-trait`); the
//! composition root wires concrete adapters as generics, so the traits need not be dyn-compatible.

use std::{future::Future, path::PathBuf};

use tailor_config::{Arch, BaseSource, ToolchainEntry};
use tokio_util::sync::CancellationToken;

use crate::{
    domain::Cell,
    error::{ExecError, ResolveError},
};

/// The Image Customizer execution port: run IC in a container for one cell, end to end.
pub trait Executor: Send + Sync {
    /// Execute one matrix cell, returning the produced artifact on success.
    fn execute(
        &self,
        cell: &Cell,
        context: &ExecutionContext,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<ExecutionResult, ExecError>> + Send;

    /// Remove outputs for the given paths — sudo-free via the ownership janitor (§7.7).
    fn clean(
        &self,
        paths: &[PathBuf],
        runtime: &RuntimeConfig,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<(), ExecError>> + Send;
}

/// Everything an executor needs to run one cell that the orchestrator resolves up front.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    /// Where artifacts are written (`<workspace>/artifacts` by default).
    pub output_dir: PathBuf,
    /// The pinned toolchain image, as `container@sha256:…`.
    pub ic_image_ref: String,
    /// The container platform, `linux/<arch>`.
    pub platform: String,
    /// `Some(i)` under `build --clones N`; suffixes all per-clone paths so clones never race.
    pub clone_index: Option<u32>,
    /// Print the resolved IC argument vector without running.
    pub dry_run: bool,
    /// Runtime knobs (path translation root, privilege, janitor image, …).
    pub runtime: RuntimeConfig,
}

/// The result of one cell execution.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// The produced artifact (a file, or a directory for `pxe-dir`).
    pub artifact_path: PathBuf,
    /// The Image Customizer process exit code.
    pub exit_code: i64,
    /// Trailing log lines, for error reporting.
    pub logs: String,
}

/// Resolved runtime settings for the bollard execution layer (`meta/docs/design.md` §5.1, §7).
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// The single source of truth for path translation and the `-v /:<hostRoot>` bind.
    pub host_root: PathBuf,
    /// Whether to run the IC container privileged.
    pub privileged: bool,
    /// Scratch directory for working copies and RPM farms.
    pub build_dir: Option<PathBuf>,
    /// IC `--log-level`.
    pub log_level: Option<String>,
    /// Host directory forwarded as IC `--image-cache-dir`.
    pub image_cache_dir: Option<PathBuf>,
    /// The sudo-free janitor image, `container@sha256:…`.
    pub janitor_image: String,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            host_root: PathBuf::from("/host"),
            privileged: true,
            build_dir: None,
            log_level: None,
            image_cache_dir: None,
            janitor_image: String::new(),
        }
    }
}

/// Low-level container runtime operations (the bollard abstraction).
pub trait ContainerRuntime: Send + Sync {
    fn pull_image(&self, reference: &str) -> impl Future<Output = Result<(), ExecError>> + Send;

    fn create_and_run(
        &self,
        config: ContainerConfig,
        cancel: CancellationToken,
    ) -> impl Future<Output = Result<ContainerResult, ExecError>> + Send;

    fn daemon_info(&self) -> impl Future<Output = Result<DaemonInfo, ExecError>> + Send;
}

/// A request to create and run one container.
#[derive(Debug, Clone)]
pub struct ContainerConfig {
    pub image_ref: String,
    pub platform: String,
    pub name: String,
    pub args: Vec<String>,
    pub binds: Vec<String>,
    pub privileged: bool,
}

/// The outcome of a container run.
#[derive(Debug, Clone)]
pub struct ContainerResult {
    pub exit_code: i64,
    pub logs: String,
}

/// Daemon configuration relevant to ownership translation (userns-remap, rootless).
#[derive(Debug, Clone, Default)]
pub struct DaemonInfo {
    pub rootless: bool,
    pub userns_remap: bool,
}

/// Resolve base images and toolchain containers to digest-pinned references plus content hashes.
pub trait BaseResolver: Send + Sync {
    fn resolve(
        &self,
        source: &BaseSource,
        arch: Arch,
    ) -> impl Future<Output = Result<ResolvedBase, ResolveError>> + Send;

    fn resolve_toolchain(
        &self,
        toolchain: &ToolchainEntry,
    ) -> impl Future<Output = Result<String, ResolveError>> + Send;
}

/// A resolved base image: a local-file content hash, or a registry digest pin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedBase {
    /// A local `path:` base — hashed to *detect* drift (not re-fetchable; not in the lock).
    LocalFile { sha256: [u8; 32], size: u64 },
    /// A registry (`oci`/`azureLinux`) base — digest-pinned and recorded in the lock.
    Oci {
        reference: String,
        platform: String,
        digest: String,
    },
}

/// Filesystem operations that need special handling (RPM farm, working copy).
pub trait FilesystemOps: Send + Sync {
    /// Build an **adjacent** reflink/hardlink/copy farm for an `rpmSources` directory, skipping any
    /// existing `repodata/` (`meta/docs/design.md` §7.8).
    fn build_rpm_farm(
        &self,
        source: &std::path::Path,
        dest: &std::path::Path,
    ) -> Result<(), std::io::Error>;

    /// Write the working-copy IC config (with injected `previewFeatures`).
    fn write_working_copy(
        &self,
        content: &[u8],
        path: &std::path::Path,
    ) -> Result<(), std::io::Error>;
}
