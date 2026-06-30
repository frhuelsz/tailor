//! Port traits — the hexagonal boundary `tailor-core` defines and adapters implement
//! (`meta/docs/architecture.md` §4.2). Traits use return-position `impl Future` (no `async-trait`); the
//! composition root wires concrete adapters as generics, so the traits need not be dyn-compatible.

use std::{
    future::Future,
    path::{Path, PathBuf},
};

use tailor_config::{Arch, BaseImageSource, BaseSource, ToolchainEntry};
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
    /// The digest-pinned IC `--image` reference for a registry base (`oci:<repo>@sha256:…`), or
    /// `None` for a local-file base (which uses `--image-file`) and for `--dry-run` (no digest
    /// resolved). Threaded from the resolved base so registry builds are reproducible
    /// (`meta/docs/design.md` §5.2/§6).
    pub base_ref: Option<String>,
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
    /// IC `--log-level` (when unset, the executor defaults IC to `debug` — `meta/docs/logging.md` §5.1).
    pub log_level: Option<String>,
    /// Host directory forwarded as IC `--image-cache-dir`.
    pub image_cache_dir: Option<PathBuf>,
    /// Opt-in host directory for per-cell IC debug logs (`--log-dir`/`TAILOR_LOG_DIR`/`runtime.logDir`).
    /// `None` (the default) means nothing is written to disk (`meta/docs/logging.md` §5.5).
    pub log_dir: Option<PathBuf>,
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
            log_dir: None,
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
    /// The cell slug, tagged onto every re-emitted IC log event (`cell = <slug>`); empty for
    /// non-cell containers such as the ownership janitor (`meta/docs/logging.md` §5.3).
    pub cell_slug: String,
    /// Host path of the per-cell on-disk IC log, when persistence is enabled — used only to point at
    /// it in the failure dump (IC itself writes the file via `--log-file`; `meta/docs/logging.md` §5.4-§5.5).
    pub log_file: Option<PathBuf>,
}

/// The outcome of a container run.
#[derive(Debug, Clone)]
pub struct ContainerResult {
    pub exit_code: i64,
    /// The captured container output, joined verbatim (used for non-IC error reporting).
    pub logs: String,
    /// On a non-zero exit, the categorized failure dump (cause + bounded context tail + optional
    /// on-disk pointer) built from the in-memory capture (`meta/docs/logging.md` §5.4). `None` on success.
    pub failure_dump: Option<String>,
}

/// Daemon configuration relevant to ownership translation (userns-remap, rootless).
#[derive(Debug, Clone, Default)]
pub struct DaemonInfo {
    pub rootless: bool,
    pub userns_remap: bool,
}

/// Resolve base images and toolchain containers to digest-pinned references plus content hashes.
pub trait BaseResolver: Send + Sync {
    /// Resolve a base source. A relative [`BaseSource::Path`] is authored relative to `image_dir`
    /// (the folder holding `image.yaml`), so it is resolved against it — never the process CWD —
    /// matching how the Image Customizer `--image-file` argument is built.
    fn resolve(
        &self,
        source: &BaseSource,
        arch: Arch,
        image_dir: &Path,
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

/// Acquire a base-image catalogue slot's file from its remote source (`meta/docs/base-image-catalogue.md`
/// §5.2/§8). `tailor bases download` drives this; the build itself never pulls — it consumes the slot
/// file like any `path` base. The fetcher streams the artifact for `linux/<arch>` to `dest`.
pub trait BaseImageFetcher: Send + Sync {
    fn fetch(
        &self,
        source: &BaseImageSource,
        arch: Arch,
        dest: &Path,
    ) -> impl Future<Output = Result<FetchedBase, ResolveError>> + Send;
}

/// Provenance recorded after a successful slot pull: the source manifest digest, plus the written
/// file's content hash and size — the lock pins the file; the digest makes the pull auditable (§5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchedBase {
    pub source_digest: String,
    pub sha256: [u8; 32],
    pub size: u64,
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
