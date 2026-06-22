//! Verb dispatch and the composition root — wires concrete adapters into the core orchestrator
//! and implements each CLI verb (`meta/docs/design.md` §11).

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;
use tailor_config::{
    Arch, BaseSource, OutputFormat, OutputSpec, RenderedCell, ToolConfig, Workspace, discover,
    expand, render_image, write_golden,
};
use tailor_core::{
    BaseResolver, BuildOptions, Executor, LockedBase, LockedContainer, Lockfile, Orchestrator,
    ResolvedBase, Selector, Target, cells_selected, runtime_config,
};
use tailor_exec::{BollardRuntime, IcExecutor};
use tailor_resolve::OciResolver;
use tokio_util::sync::CancellationToken;

use crate::{
    cli::{BuildArgs, Cli, Command, MatrixFormat},
    error::AppError,
};

const LOCK_FILE: &str = "tailor.lock";
const ARTIFACTS_DIR: &str = "artifacts";

/// Load the workspace, then dispatch the requested verb.
pub(crate) async fn dispatch(cli: Cli) -> Result<(), AppError> {
    // `version` needs no workspace — handle it before discovery so it works anywhere.
    if matches!(cli.command, Command::Version) {
        print_version();
        return Ok(());
    }
    let workspace = load_workspace(&cli)?;
    match cli.command {
        Command::Version => unreachable!("handled before workspace discovery"),
        Command::List => {
            list(&workspace);
            Ok(())
        }
        Command::Show { image, field } => show(&workspace, &image, field.as_deref()),
        Command::Validate(args) => {
            validate(&workspace, &args.images, &selector(&args.select, &[])?)
        }
        Command::Render(args) => render(&workspace, &args.images, &selector(&args.select, &[])?),
        Command::Explain { image, select } => explain(&workspace, &image, &selector(&select, &[])?),
        Command::Matrix(args) => matrix(
            &workspace,
            &args.images,
            &selector(&args.select, &[])?,
            args.format,
        ),
        Command::Slugs(args) => matrix(
            &workspace,
            &args.images,
            &selector(&args.select, &[])?,
            MatrixFormat::Slugs,
        ),
        Command::Resolve(args) => resolve(&workspace, &args.images).await,
        Command::Lock | Command::Update => lock(&workspace).await,
        Command::Build(args) => build(&workspace, args).await,
        Command::Clean(args) => {
            clean(&workspace, &args.images, &selector(&args.select, &[])?).await
        }
    }
}

/// Build a [`Selector`] from the shared `-s/--cell` flags plus any verb-specific `--arch` values.
fn selector(args: &crate::cli::SelectArgs, arches: &[String]) -> Result<Selector, AppError> {
    Ok(Selector::parse(&args.select, &args.cell, arches)?)
}

/// Print the version exactly as `--version` does — clap renders both from the same source.
fn print_version() {
    use clap::CommandFactory;
    print!("{}", Cli::command().render_version());
}

fn load_workspace(cli: &Cli) -> Result<Workspace, AppError> {
    let start = match &cli.manifest {
        Some(path) if path.is_file() => path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
        Some(path) => path.clone(),
        None => {
            std::env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?
        }
    };
    Ok(discover(start)?)
}

// ───────────────────────────── pure verbs (no Docker / network) ─────────────────────────────

fn list(workspace: &Workspace) {
    println!("Images:");
    for image in &workspace.images {
        let cells = image
            .definition
            .matrix
            .as_ref()
            .map_or(1, |m| expand(m).map_or(0, |c| c.len()));
        println!("  {:<28} {cells} cell(s)", image.definition.name);
    }
    if let Some(tool) = &workspace.tool {
        println!("\nToolchains (default: {}):", tool.toolchains.default);
        for (id, entry) in &tool.toolchains.entries {
            println!("  {id:<10} {}:{}", entry.container, entry.effective_tag());
        }
    }
}

fn show(workspace: &Workspace, name: &str, field: Option<&str>) -> Result<(), AppError> {
    let image = workspace
        .image(name)
        .ok_or_else(|| AppError::Message(format!("unknown image `{name}`")))?;
    let definition = &image.definition;
    if let Some(field) = field {
        let value = match field {
            "name" => definition.name.clone(),
            "dir" => image.dir.display().to_string(),
            "outputs" => format!("{:?}", definition.outputs),
            "features" => definition.features.join(", "),
            other => return Err(AppError::Message(format!("unknown field `{other}`"))),
        };
        println!("{value}");
        return Ok(());
    }
    println!("name:    {}", definition.name);
    println!("dir:     {}", image.dir.display());
    if let Some(matrix) = &definition.matrix {
        let cells = expand(matrix).map_or(0, |c| c.len());
        println!(
            "matrix:  {cells} cell(s) across {} axis(es)",
            matrix.axes.len()
        );
        let width = matrix.axes.keys().map(String::len).max().unwrap_or(0);
        for (axis, values) in &matrix.axes {
            println!("  {axis:<width$} : {}", values.join(", "));
        }
        if !matrix.exclude.is_empty() {
            println!("  ({} exclude rule(s))", matrix.exclude.len());
        }
    } else {
        println!("matrix:  none (single cell)");
    }
    let outputs: Vec<&str> = definition
        .outputs
        .iter()
        .flatten()
        .map(|o| o.format.as_str())
        .collect();
    if !outputs.is_empty() {
        println!("outputs: {}", outputs.join(", "));
    }
    if !definition.features.is_empty() {
        println!("features: {}", definition.features.join(", "));
    }
    Ok(())
}

fn validate(workspace: &Workspace, names: &[String], selector: &Selector) -> Result<(), AppError> {
    for target in build_targets(workspace, names)? {
        let cells = cells_selected(&target, selector)?;
        println!("✓ {:<28} {} cell(s) valid", target.name(), cells.len());
    }
    Ok(())
}

fn render(workspace: &Workspace, names: &[String], selector: &Selector) -> Result<(), AppError> {
    for target in build_targets(workspace, names)? {
        for cell in cells_selected(&target, selector)? {
            let path = write_golden(&target.dir, cell.slug.as_ref(), &cell.ic_config)?;
            println!("rendered {}", path.display());
        }
    }
    Ok(())
}

fn explain(workspace: &Workspace, name: &str, selector: &Selector) -> Result<(), AppError> {
    let targets = build_targets(workspace, std::slice::from_ref(&name.to_owned()))?;
    let target = targets
        .first()
        .ok_or_else(|| AppError::Message(format!("unknown image `{name}`")))?;
    let cells = cells_selected(target, selector)?;
    println!("{name}: {} cell(s)\n", cells.len());
    for cell in &cells {
        println!("── {} ──", cell.slug.as_ref());
        let yaml = serde_yaml_ng::to_string(&cell.ic_config)
            .map_err(|e| AppError::Message(format!("serialize: {e}")))?;
        println!("{yaml}");
    }
    // NOTE: per-fragment provenance (the source map for "which fragment set this value") is future work.
    Ok(())
}

#[derive(Serialize)]
struct MatrixEntry {
    image: String,
    slug: String,
    axes: BTreeMap<String, String>,
    format: String,
}

fn matrix(
    workspace: &Workspace,
    names: &[String],
    selector: &Selector,
    format: MatrixFormat,
) -> Result<(), AppError> {
    let mut entries = Vec::new();
    for target in build_targets(workspace, names)? {
        for cell in cells_selected(&target, selector)? {
            entries.push(MatrixEntry {
                image: target.name().to_owned(),
                slug: cell.slug.to_string(),
                axes: cell.axes.clone(),
                format: cell.output.format.as_str().to_owned(),
            });
        }
    }
    match format {
        MatrixFormat::Json => println!("{}", serde_json::to_string_pretty(&entries)?),
        MatrixFormat::Slugs => {
            for entry in &entries {
                println!("{}", entry.slug);
            }
        }
    }
    Ok(())
}

// ───────────────────────────── network verbs (registry resolution) ─────────────────────────────

async fn resolve(workspace: &Workspace, names: &[String]) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, names)?;
    let lock = build_lock(&tool, &targets, &OciResolver::new()).await?;
    let yaml = serde_yaml_ng::to_string(&lock)
        .map_err(|e| AppError::Message(format!("serialize lock: {e}")))?;
    print!("{yaml}");
    Ok(())
}

async fn lock(workspace: &Workspace) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, &[])?;
    let lock = build_lock(&tool, &targets, &OciResolver::new()).await?;
    let path = workspace.root.join(LOCK_FILE);
    lock.write(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}

async fn build_lock(
    tool: &ToolConfig,
    targets: &[Arc<Target>],
    resolver: &OciResolver,
) -> Result<Lockfile, AppError> {
    let mut lock = Lockfile::default();
    for (id, entry) in &tool.toolchains.entries {
        let digest = resolver.resolve_toolchain(entry).await?;
        lock.toolchains.insert(
            id.clone(),
            LockedContainer {
                container: entry.container.clone(),
                version: entry.version.as_ref().map(ToString::to_string),
                tag: Some(entry.effective_tag()),
                digest,
            },
        );
    }
    for target in targets {
        for cell in render_image(&target.definition, &target.dir)? {
            if !matches!(
                cell.base,
                BaseSource::Oci { .. } | BaseSource::AzureLinux { .. }
            ) {
                continue;
            }
            for arch in cell_arches(&cell, target) {
                if let ResolvedBase::Oci {
                    reference,
                    platform,
                    digest,
                } = resolver.resolve(&cell.base, arch).await?
                {
                    lock.upsert_base(LockedBase {
                        reference,
                        platform,
                        digest,
                    });
                }
            }
        }
    }
    Ok(lock)
}

// ───────────────────────────── execution verbs (Docker) ─────────────────────────────

async fn build(workspace: &Workspace, args: BuildArgs) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, &args.images)?;
    let selection = selector(&args.select, &args.arch)?;
    let output_dir = args
        .output_dir
        .clone()
        .unwrap_or_else(|| workspace.root.join(ARTIFACTS_DIR));
    let orchestrator = Orchestrator::new(
        IcExecutor::new(BollardRuntime::connect()?),
        OciResolver::new(),
    );

    // Dry-run prints each selected cell's container invocation without resolving digests or running.
    if args.dry_run {
        let results = orchestrator
            .dry_run(&targets, &tool, &selection, &output_dir)
            .await?;
        println!("{} cell(s) (dry-run)\n", results.len());
        for result in results {
            println!("{}\n", result.logs);
        }
        return Ok(());
    }

    let lock = Lockfile::read(&workspace.root.join(LOCK_FILE))?;
    let plan = orchestrator
        .plan(&targets, &tool, &lock, &selection, &output_dir)
        .await?;
    let stale = plan.stale().count();
    println!("{} cell(s) planned, {stale} to build", plan.cells.len());

    let clones = args.clones.max(1);
    for clone in 0..clones {
        let options = BuildOptions {
            force: args.force,
            dry_run: false,
            clone_index: (clones > 1).then_some(clone),
        };
        let results = orchestrator
            .build(
                &plan,
                &tool,
                &lock,
                &output_dir,
                &options,
                CancellationToken::new(),
            )
            .await?;
        for result in results {
            println!(
                "  built {} (exit {})",
                result.artifact_path.display(),
                result.exit_code
            );
        }
    }
    Ok(())
}

async fn clean(
    workspace: &Workspace,
    names: &[String],
    selector: &Selector,
) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, names)?;
    let output_dir = workspace.root.join(ARTIFACTS_DIR);
    let lock = Lockfile::read(&workspace.root.join(LOCK_FILE))?;

    let mut paths = Vec::new();
    for target in &targets {
        for cell in cells_selected(target, selector)? {
            paths.push(output_dir.join(tailor_core::artifact_name(
                cell.slug.as_ref(),
                cell.output.format,
            )));
        }
    }

    let executor = IcExecutor::new(BollardRuntime::connect()?);
    executor
        .clean(
            &paths,
            &runtime_config(&tool, &lock),
            CancellationToken::new(),
        )
        .await?;
    println!("cleaned {} artifact path(s)", paths.len());
    Ok(())
}

// ───────────────────────────── helpers ─────────────────────────────

fn tool_config(workspace: &Workspace) -> ToolConfig {
    match &workspace.tool {
        Some(tool) => tool.clone(),
        None => tailor_config::defaults::default_tool_config(),
    }
}

fn build_targets(workspace: &Workspace, names: &[String]) -> Result<Vec<Arc<Target>>, AppError> {
    let defaults = workspace.tool.as_ref().and_then(|t| t.defaults.as_ref());
    let default_arches = defaults
        .and_then(|d| d.architectures.clone())
        .unwrap_or_else(|| vec![Arch::Amd64]);
    let default_outputs = defaults
        .and_then(|d| d.outputs.clone())
        .unwrap_or_else(default_cosi);

    let mut targets = Vec::new();
    for image in &workspace.images {
        if !names.is_empty() && !names.iter().any(|n| n == &image.definition.name) {
            continue;
        }
        let architectures = image
            .definition
            .architectures
            .clone()
            .unwrap_or_else(|| default_arches.clone());
        targets.push(Arc::new(Target {
            definition: image.definition.clone(),
            dir: image.dir.clone(),
            architectures,
            default_outputs: default_outputs.clone(),
        }));
    }
    if !names.is_empty() && targets.is_empty() {
        return Err(AppError::Message(format!(
            "no matching images: {}",
            names.join(", ")
        )));
    }
    Ok(targets)
}

/// The built-in default output (`cosi`) when neither the image nor the workspace declares one.
fn default_cosi() -> Vec<OutputSpec> {
    vec![OutputSpec {
        format: OutputFormat::Cosi,
        cosi_compression_level: None,
        name: None,
    }]
}

fn cell_arches(cell: &RenderedCell, target: &Target) -> Vec<Arch> {
    match cell.tuple.get("arch").and_then(parse_arch) {
        Some(arch) => vec![arch],
        None => target.architectures.clone(),
    }
}

fn parse_arch(value: &str) -> Option<Arch> {
    match value {
        "amd64" => Some(Arch::Amd64),
        "arm64" => Some(Arch::Arm64),
        _ => None,
    }
}
