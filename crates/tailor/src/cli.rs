//! The clap command-line surface (`meta/docs/design.md` §11).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Full version string: the Cargo version plus SemVer build metadata (commit + build date),
/// computed by `build.rs` and shared by `--version` and the `version` subcommand.
pub(crate) const VERSION: &str = env!("TAILOR_VERSION");

/// Cargo-style, manifest-driven front-end for the Azure Linux Image Customizer.
#[derive(Debug, Parser)]
#[command(name = "tailor", version = VERSION, about)]
pub(crate) struct Cli {
    /// Path to `tailor.yaml` (default: walk up from the current directory, like Cargo).
    #[arg(long, global = true)]
    pub(crate) manifest: Option<PathBuf>,

    /// Promote authority/confinement warnings to errors.
    #[arg(long, global = true)]
    pub(crate) strict: bool,

    /// Increase verbosity (repeatable).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,

    /// Decrease verbosity (repeatable).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) quiet: u8,

    /// Container engine for this invocation: `docker` (default), `podman`, or `auto`. Overrides
    /// `runtime.engine` in `tailor.yaml` (`meta/docs/container-runtimes.md` §3).
    #[arg(long, global = true, value_enum)]
    pub(crate) engine: Option<EngineArg>,

    /// Container engine endpoint for this invocation, e.g. `unix:///run/user/1000/podman/podman.sock`
    /// or `tcp://host:2375`. Overrides `runtime.host` and `DOCKER_HOST` / `CONTAINER_HOST`.
    #[arg(long, global = true, value_name = "ENDPOINT")]
    pub(crate) host: Option<String>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// The container engine selector for `--engine` (mirrors [`tailor_config::Engine`]).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum EngineArg {
    /// The Docker daemon (default).
    Docker,
    /// Podman via its Docker-compatible API.
    Podman,
    /// Probe known Docker/Podman sockets and use the first that answers.
    Auto,
}

impl From<EngineArg> for tailor_config::Engine {
    fn from(value: EngineArg) -> Self {
        match value {
            EngineArg::Docker => tailor_config::Engine::Docker,
            EngineArg::Podman => tailor_config::Engine::Podman,
            EngineArg::Auto => tailor_config::Engine::Auto,
        }
    }
}

/// The verb to run.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Resolve and run Image Customizer for the given images (default: all).
    Build(BuildArgs),
    /// Render the final IC config per cell, writing golden snapshots.
    Render(ImagesArgs),
    /// List images (and toolchains).
    List,
    /// Show resolved information for one image.
    Show {
        image: String,
        field: Option<String>,
    },
    /// Remove generated artifacts and build stamps (sudo-free via the janitor).
    Clean(ImagesArgs),
    /// Resolve digests/hashes without building.
    Resolve(ImagesArgs),
    /// Write `tailor.lock` without building.
    Lock,
    /// Re-resolve and rewrite `tailor.lock`.
    Update,
    /// Validate image definitions (renders every cell) without building.
    Validate(ImagesArgs),
    /// Show matrix expansion and the rendered config per cell.
    Explain {
        image: String,
        #[command(flatten)]
        select: SelectArgs,
    },
    /// Emit the build matrix (all viable cells) for the selected images.
    Matrix(MatrixArgs),
    /// Print one cell slug per line for the selected images (same as `matrix --format slugs`).
    Slugs(ImagesArgs),
    /// Print version information (identical to `--version`).
    Version,
    /// Scaffold a new tailor project (manifest and/or image definition).
    Init(InitArgs),
    /// Add an image or axis to an existing tailor project.
    Add {
        #[command(subcommand)]
        what: AddCommand,
    },
}

/// Subcommands of `tailor add`.
#[derive(Debug, Subcommand)]
pub(crate) enum AddCommand {
    /// Scaffold a new member image and register it in the workspace `tailor.yaml`.
    Image {
        /// The new image's name (also its directory, created in the current directory).
        name: String,
    },
    /// Append a new axis to an image's matrix and create its `by-<axis>/` directory.
    ///
    /// Pass just `<axis>` when the workspace has one image, or `<image> <axis>` to choose one.
    Axis {
        /// The axis name, or the image name when a second argument is given.
        first: String,
        /// The axis name (when the first argument is the image name).
        second: Option<String>,
    },
}

/// Args for `tailor init`.
#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// The image name (also the member directory for the `base`/`advanced` templates).
    pub(crate) name: String,

    /// Which scaffold to generate (default: `base`).
    #[arg(value_enum, default_value_t = InitTemplate::Base)]
    pub(crate) template: InitTemplate,
}

/// Scaffold template for `tailor init`.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub(crate) enum InitTemplate {
    /// A workspace `tailor.yaml` plus a basic `<name>/image.yaml`.
    #[default]
    Base,
    /// A single standalone `image.yaml` in the current directory (no `tailor.yaml`).
    Simple,
    /// Like `base`, but the image declares two example axes with `by-*` fragments.
    Advanced,
}

/// Args for verbs that operate on a set of images and (optionally) a cell selection.
#[derive(Debug, Args)]
pub(crate) struct ImagesArgs {
    /// Image names to operate on (default: all in the workspace).
    pub(crate) images: Vec<String>,

    #[command(flatten)]
    pub(crate) select: SelectArgs,
}

/// Reusable cell-selection flags (`meta/docs/design.md` §11). Pin some axes for a slice, all axes for one
/// cell, or name exact cells by slug. Axis values are `[A-Za-z0-9.-]+`, so `,` and `=` are safe
/// delimiters.
#[derive(Debug, Args)]
pub(crate) struct SelectArgs {
    /// Constrain matrix axes, e.g. `-s variant=grub,arch=amd64` (repeatable). Unset axes expand
    /// fully, so `-s arch=amd64` builds every amd64 cell.
    #[arg(short, long = "select", value_name = "AXIS=VALUE")]
    pub(crate) select: Vec<String>,

    /// Select exact cells by slug (repeatable); matches the `slug` field of `tailor matrix` output.
    #[arg(long = "cell", value_name = "SLUG")]
    pub(crate) cell: Vec<String>,
}

/// Output format for `tailor matrix`.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum MatrixFormat {
    /// A JSON array of cell objects (`image`, `slug`, `axes`, `format`).
    Json,
    /// One cell slug per line — feeds `tailor build --cell <slug>` directly.
    Slugs,
}

/// Args for `tailor matrix`.
#[derive(Debug, Args)]
pub(crate) struct MatrixArgs {
    /// Image names to enumerate (default: all in the workspace).
    pub(crate) images: Vec<String>,

    #[command(flatten)]
    pub(crate) select: SelectArgs,

    /// How to print the matrix.
    #[arg(long, value_enum, default_value_t = MatrixFormat::Json)]
    pub(crate) format: MatrixFormat,
}

/// Args for `tailor build`.
#[derive(Debug, Args)]
pub(crate) struct BuildArgs {
    /// Image names to build (default: all).
    pub(crate) images: Vec<String>,

    #[command(flatten)]
    pub(crate) select: SelectArgs,

    /// Require a complete `tailor.lock`; fail on a missing entry or drift.
    #[arg(long)]
    pub(crate) locked: bool,

    /// Ignore incremental up-to-date checks.
    #[arg(long)]
    pub(crate) force: bool,

    /// Restrict the build to the given architecture(s).
    #[arg(long)]
    pub(crate) arch: Vec<String>,

    /// Where to write artifacts (default: `<workspace>/artifacts`).
    #[arg(long)]
    pub(crate) output_dir: Option<PathBuf>,

    /// Render every selected cell's container invocation without running it.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Max parallel matrix cells (reserved; currently sequential).
    #[arg(long)]
    pub(crate) jobs: Option<usize>,

    /// Build N identical clones of each cell.
    #[arg(long, default_value_t = 1)]
    pub(crate) clones: u32,
}
