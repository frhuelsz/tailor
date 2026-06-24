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
    expand, find_manifest, render_image, write_golden,
};
use tailor_core::{
    BaseResolver, BuildOptions, Executor, LockedBase, LockedContainer, Lockfile, Orchestrator,
    ResolvedBase, Selector, SigningRequirement, Target, cells_selected, preflight_profile,
    runtime_config,
};
use tailor_exec::{BollardRuntime, IcExecutor, NoopRuntime, ResolveInputs, resolve};
use tailor_resolve::OciResolver;
use tokio_util::sync::CancellationToken;

use crate::{
    cli::{AddCommand, BuildArgs, Cli, Command, InitArgs, InitTemplate, MatrixFormat},
    error::AppError,
    scaffold,
};

const LOCK_FILE: &str = "tailor.lock";
const ARTIFACTS_DIR: &str = "artifacts";

/// A per-invocation engine/endpoint override from the global `--engine` / `--host` flags
/// (`meta/docs/container-runtimes.md` §3). Both sit above the manifest in the precedence ladder.
#[derive(Clone, Default)]
pub(crate) struct EngineOverride {
    engine: Option<tailor_config::Engine>,
    host: Option<String>,
}

/// Load the workspace, then dispatch the requested verb.
pub(crate) async fn dispatch(cli: Cli) -> Result<(), AppError> {
    // `version` and `init` need no workspace — handle them before discovery so they work anywhere
    // (`init` is what creates a workspace in the first place).
    match &cli.command {
        Command::Version => {
            print_version();
            return Ok(());
        }
        Command::Init(args) => return init(args),
        Command::Add { what } => return add(what),
        _ => {}
    }
    let workspace = load_workspace(&cli)?;
    let engine = EngineOverride {
        engine: cli.engine.map(Into::into),
        host: cli.host.clone(),
    };
    match cli.command {
        Command::Version | Command::Init(_) | Command::Add { .. } => {
            unreachable!("handled before workspace discovery")
        }
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
        Command::Resolve(args) => resolve_verb(&workspace, &args.images).await,
        Command::Lock | Command::Update => lock(&workspace).await,
        Command::Build(args) => build(&workspace, args, &engine).await,
        Command::Clean(args) => {
            clean(
                &workspace,
                &args.images,
                &selector(&args.select, &[])?,
                &engine,
            )
            .await
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
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, names)?;
    for target in &targets {
        let cells = cells_selected(target, selector)?;
        println!("✓ {:<28} {} cell(s) valid", target.name(), cells.len());
    }
    // Surface signing prerequisites non-fatally (meta/docs/signing.md §5.1) so they are discoverable
    // without starting a real build.
    report_signing(
        &signing_requirements(&targets, tool.signing.as_ref())?,
        &workspace.root,
    );
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

async fn resolve_verb(workspace: &Workspace, names: &[String]) -> Result<(), AppError> {
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

async fn build(
    workspace: &Workspace,
    args: BuildArgs,
    engine: &EngineOverride,
) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, &args.images)?;
    let selection = selector(&args.select, &args.arch)?;
    let output_dir = args
        .output_dir
        .clone()
        .unwrap_or_else(|| workspace.root.join(ARTIFACTS_DIR));
    let signing = signing_requirements(&targets, tool.signing.as_ref())?;

    // Dry-run prints each selected cell's container invocation without resolving digests, running,
    // or contacting any engine — so it stays daemon-free via a no-op runtime.
    if args.dry_run {
        let orchestrator = Orchestrator::new(IcExecutor::new(NoopRuntime), OciResolver::new());
        let results = orchestrator
            .dry_run(&targets, &tool, &selection, &output_dir)
            .await?;
        println!("{} cell(s) (dry-run)\n", results.len());
        for result in results {
            println!("{}\n", result.logs);
        }
        if !signing.is_empty() {
            report_signing(&signing, &workspace.root);
            println!(
                "note: signing execution is not yet implemented; this dry-run shows the unsigned \
                 customize invocation (meta/docs/signing.md §11)."
            );
        }
        return Ok(());
    }

    // Fail fast on signing prerequisites *before* any IC run (meta/docs/signing.md §5.1); then, since
    // signing execution is not yet wired, refuse rather than emit a silently-unsigned image.
    if !signing.is_empty() {
        tailor_core::preflight(&signing, &workspace.root)?;
        return Err(signing_not_implemented(&signing));
    }

    // A real build contacts the engine: resolve it and fail fast now if it is missing or
    // unreachable (`meta/docs/container-runtimes.md` §4-§5).
    let orchestrator = Orchestrator::new(
        IcExecutor::new(establish_runtime(engine, &tool).await?),
        OciResolver::new(),
    );

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
    engine: &EngineOverride,
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

    let executor = IcExecutor::new(establish_runtime(engine, &tool).await?);
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

/// Gather the connection-resolution inputs (`meta/docs/container-runtimes.md` §3): the per-invocation
/// flags, the engine environment variables, and the manifest `runtime.engine` / `runtime.host`.
fn resolve_inputs(engine: &EngineOverride, tool: &ToolConfig) -> ResolveInputs {
    let runtime = tool.runtime.as_ref();
    ResolveInputs {
        flag_engine: engine.engine,
        flag_host: engine.host.clone(),
        env_docker_host: non_empty_env("DOCKER_HOST"),
        env_container_host: non_empty_env("CONTAINER_HOST"),
        manifest_engine: runtime.and_then(|runtime| runtime.engine),
        manifest_host: runtime.and_then(|runtime| runtime.host.clone()),
        xdg_runtime_dir: non_empty_env("XDG_RUNTIME_DIR"),
    }
}

/// Resolve the engine/endpoint, connect, and run the fail-fast preflight (§4-§5).
async fn establish_runtime(
    engine: &EngineOverride,
    tool: &ToolConfig,
) -> Result<BollardRuntime, AppError> {
    let plan = resolve(&resolve_inputs(engine, tool));
    Ok(BollardRuntime::establish(&plan).await?)
}

/// A set, non-empty environment variable, else `None`.
fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

// ───────────────────────────── signing (meta/docs/signing.md) ─────────────────────────────

/// Gather the distinct signing profiles the selected images require, tracking which images need
/// each (`meta/docs/signing.md` §5.1). Resolution also validates each referenced profile.
fn signing_requirements<'a>(
    targets: &[Arc<Target>],
    workspace_signing: Option<&'a tailor_config::SigningConfig>,
) -> Result<Vec<SigningRequirement<'a>>, AppError> {
    let mut requirements: Vec<SigningRequirement<'a>> = Vec::new();
    for target in targets {
        let resolved =
            tailor_config::resolve_signing(target.definition.signing.as_ref(), workspace_signing)?;
        if let Some((profile_id, profile)) = resolved {
            let image = target.definition.name.clone();
            if let Some(existing) = requirements
                .iter_mut()
                .find(|requirement| requirement.profile_id == profile_id)
            {
                existing.images.push(image);
            } else {
                requirements.push(SigningRequirement {
                    profile_id,
                    profile,
                    images: vec![image],
                });
            }
        }
    }
    Ok(requirements)
}

/// Report signing-preflight status **without failing** — for `validate` and `--dry-run`
/// (`meta/docs/signing.md` §5.1). Relative key/cert paths resolve against `base_dir`.
fn report_signing(requirements: &[SigningRequirement<'_>], base_dir: &Path) {
    for requirement in requirements {
        let unmet = preflight_profile(requirement.profile, base_dir);
        if unmet.is_empty() {
            println!(
                "✓ signing profile `{}` ready (image(s): {})",
                requirement.profile_id,
                requirement.images.join(", ")
            );
        } else {
            for detail in unmet {
                println!(
                    "⚠ signing profile `{}` not ready: {} (image(s): {})",
                    requirement.profile_id,
                    detail,
                    requirement.images.join(", ")
                );
            }
        }
    }
}

/// The hard error returned for a signed build while signing **execution** is not yet implemented
/// (`meta/docs/signing.md` §11). tailor refuses rather than emit a silently-unsigned image.
fn signing_not_implemented(requirements: &[SigningRequirement<'_>]) -> AppError {
    let profiles: Vec<&str> = requirements
        .iter()
        .map(|requirement| requirement.profile_id.as_str())
        .collect();
    AppError::Message(format!(
        "signing is requested (profile(s): {}) and its prerequisites are satisfied, but the signing \
         pipeline is not yet implemented (Milestone S1 in progress — see meta/docs/signing.md §11). \
         Refusing to build a silently-unsigned image; remove `signing:` to build unsigned.",
        profiles.join(", ")
    ))
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

// ───────────────────────────── init (scaffolding) ─────────────────────────────

const IMAGE_FILE: &str = "image.yaml";
const MANIFEST_FILE: &str = "tailor.yaml";
/// Token replaced with the image name in the templates below.
const NAME_TOKEN: &str = "__IMAGE_NAME__";

/// Scaffold a new tailor project from a template (`tailor init <name> [base|simple|advanced]`).
fn init(args: &InitArgs) -> Result<(), AppError> {
    let name = args.name.trim();
    validate_name(name)?;
    let cwd =
        std::env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;

    match args.template {
        InitTemplate::Simple => {
            // A single standalone image. The render pipeline keys off `image.yaml`, so the file is
            // named `image.yaml` (in the cwd) rather than `<name>.yaml`; the image's `name:` is set.
            write_new(&cwd.join(IMAGE_FILE), &fill_template(SIMPLE_IMAGE, name))?;
            println!("\nScaffolded standalone image `{name}`. Try: tailor validate");
        }
        InitTemplate::Base => {
            write_new(&cwd.join(MANIFEST_FILE), TAILOR_MANIFEST)?;
            write_new(
                &cwd.join(name).join(IMAGE_FILE),
                &fill_template(BASE_IMAGE, name),
            )?;
            println!("\nScaffolded workspace with image `{name}`. Try: tailor list");
        }
        InitTemplate::Advanced => {
            let image_dir = cwd.join(name);
            write_new(&cwd.join(MANIFEST_FILE), TAILOR_MANIFEST)?;
            write_new(
                &image_dir.join(IMAGE_FILE),
                &fill_template(ADVANCED_IMAGE, name),
            )?;
            write_new(
                &image_dir.join("by-variant/minimal.yaml"),
                BY_VARIANT_MINIMAL,
            )?;
            write_new(&image_dir.join("by-variant/full.yaml"), BY_VARIANT_FULL)?;
            write_new(&image_dir.join("by-arch/amd64.yaml"), BY_ARCH_AMD64)?;
            write_new(&image_dir.join("by-arch/arm64.yaml"), BY_ARCH_ARM64)?;
            println!(
                "\nScaffolded workspace with matrix image `{name}`. Try: tailor matrix {name}"
            );
        }
    }
    Ok(())
}

/// Substitute the image name into a template.
fn fill_template(template: &str, name: &str) -> String {
    template.replace(NAME_TOKEN, name)
}

/// Reject names that are empty or that would escape their directory.
fn validate_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(AppError::Message(format!(
            "invalid name `{name}`: use a bare name without path separators",
        )));
    }
    Ok(())
}

// ───────────────────────────── add (image / axis) ─────────────────────────────

/// Placeholder value seeded for a freshly-added axis so the matrix stays valid until edited.
const AXIS_PLACEHOLDER: &str = "todo";

fn add(what: &AddCommand) -> Result<(), AppError> {
    match what {
        AddCommand::Image { name } => add_image(name),
        // `add axis <axis>` (one arg) or `add axis <image> <axis>` (two args).
        AddCommand::Axis {
            first,
            second: Some(axis),
        } => add_axis(Some(first), axis),
        AddCommand::Axis {
            first: axis,
            second: None,
        } => add_axis(None, axis),
    }
}

/// `tailor add image <name>` — scaffold a new member image and register it in the workspace manifest.
fn add_image(name: &str) -> Result<(), AppError> {
    validate_name(name)?;
    let cwd =
        std::env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;
    let manifest = find_manifest(&cwd).ok_or_else(|| {
        AppError::Message(
            "no tailor.yaml in this directory or any parent — run `tailor init` first".to_owned(),
        )
    })?;
    let root = manifest.parent().unwrap_or(cwd.as_path());
    let image_dir = cwd.join(name);
    write_new(
        &image_dir.join(IMAGE_FILE),
        &fill_template(BASE_IMAGE, name),
    )?;

    let rel = image_dir
        .strip_prefix(root)
        .unwrap_or(&image_dir)
        .to_string_lossy()
        .replace('\\', "/");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| AppError::Message(format!("read {}: {e}", manifest.display())))?;
    let updated = scaffold::register_member(&text, &rel).map_err(AppError::Message)?;
    if updated != text {
        std::fs::write(&manifest, updated)
            .map_err(|e| AppError::Message(format!("write {}: {e}", manifest.display())))?;
        println!("  updated {}", manifest.display());
    }
    println!("\nAdded image `{name}`. Try: tailor list");
    Ok(())
}

/// `tailor add axis [<image>] <axis>` — append an axis to an image's matrix and create its
/// `by-<axis>/` directory.
fn add_axis(image: Option<&str>, axis: &str) -> Result<(), AppError> {
    validate_name(axis)?;
    let cwd =
        std::env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;
    let workspace = discover(cwd)?;
    let target = match image {
        Some(name) => workspace
            .image(name)
            .ok_or_else(|| AppError::Message(format!("unknown image `{name}`")))?,
        None => match workspace.images.as_slice() {
            [only] => only,
            [] => return Err(AppError::Message("no image found here".to_owned())),
            many => {
                let names = many
                    .iter()
                    .map(|i| i.definition.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(AppError::Message(format!(
                    "workspace has multiple images ({names}); name one: tailor add axis <image> {axis}",
                )));
            }
        },
    };

    let image_file = target.dir.join(IMAGE_FILE);
    let text = std::fs::read_to_string(&image_file)
        .map_err(|e| AppError::Message(format!("read {}: {e}", image_file.display())))?;
    let updated = scaffold::add_axis(&text, axis, AXIS_PLACEHOLDER).map_err(AppError::Message)?;
    std::fs::write(&image_file, updated)
        .map_err(|e| AppError::Message(format!("write {}: {e}", image_file.display())))?;
    println!("  updated {}", image_file.display());

    let by_dir = target.dir.join(format!("by-{axis}"));
    std::fs::create_dir_all(&by_dir)
        .map_err(|e| AppError::Message(format!("create {}: {e}", by_dir.display())))?;
    println!("  created {}/", by_dir.display());

    println!(
        "\nAdded axis `{axis}` to `{}` (placeholder value `{AXIS_PLACEHOLDER}`). Edit its values in \
         image.yaml and add by-{axis}/<value>.yaml fragments.",
        target.definition.name
    );
    Ok(())
}

/// Write a scaffold file, creating parent directories but refusing to overwrite an existing file.
fn write_new(path: &Path, contents: &str) -> Result<(), AppError> {
    if path.exists() {
        return Err(AppError::Message(format!(
            "refusing to overwrite existing {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Message(format!("create {}: {e}", parent.display())))?;
    }
    std::fs::write(path, contents)
        .map_err(|e| AppError::Message(format!("write {}: {e}", path.display())))?;
    println!("  created {}", path.display());
    Ok(())
}

const TAILOR_MANIFEST: &str = "\
# tailor.yaml — the workspace root (Cargo-style). `tailor` walks up from any subdirectory to find
# this file; every `*/image.yaml` beneath it is an auto-discovered member image. Repo-wide settings
# live here: the Image Customizer toolchain(s) and the defaults every image inherits.
schemaVersion: 1

toolchains:
  default: ic
  entries:
    ic:
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      # version: \"1.3.0\"   # pin a specific Image Customizer; omit to track the :latest tag

defaults:
  architectures: [amd64]
  outputs:
    - format: cosi
";

const BASE_IMAGE: &str = "\
# __IMAGE_NAME__/image.yaml — an image definition. Top-level keys are tailor's (name, base, …);
# everything under `config:` is Image Customizer config, passed through to IC untouched.
#
# Architectures and the output format are inherited from the workspace `defaults:`, and the default
# toolchain is used, so this file only declares what makes the image itself.
name: __IMAGE_NAME__

base:
  azureLinux:
    version: \"3.0\"
    variant: minimal-os

config:
  os:
    hostname: __IMAGE_NAME__
    packages:
      install:
        - openssh-server
    services:
      enable:
        - sshd
";

const SIMPLE_IMAGE: &str = "\
# image.yaml — a standalone image (there is no tailor.yaml). This single file is the whole
# definition. Top-level keys are tailor's; everything under `config:` is Image Customizer config.
#
# With no toolchain pinned, tailor uses its built-in default Image Customizer (the :latest tag).
name: __IMAGE_NAME__

outputs:
  - format: cosi

base:
  azureLinux:
    version: \"3.0\"
    variant: minimal-os

config:
  os:
    hostname: __IMAGE_NAME__
    packages:
      install:
        - openssh-server
    services:
      enable:
        - sshd
";

const ADVANCED_IMAGE: &str = "\
# __IMAGE_NAME__/image.yaml — a MATRIX image. The `matrix:` block multiplies its axes into one build
# per combination (here variant × arch = 4 cells). Per-axis deltas live in `by-<axis>/<value>.yaml`
# and merge on top of this shared config (lists append, maps deep-merge).
#
# IMPORTANT: fragments apply in the order the axes are DECLARED below — later axes win conflicts and
# append last — so you control merge precedence by ordering axes here, not by directory names.
name: __IMAGE_NAME__

matrix:
  variant: [minimal, full]
  arch:    [amd64, arm64]

outputs:
  - format: cosi

base:
  # An azureLinux (MCR) base is a multi-arch manifest, so one entry covers both arches; tailor
  # resolves the per-arch digest at pull time. (Use by-arch/<arch>.yaml `base:` for per-arch bases.)
  azureLinux:
    version: \"3.0\"
    variant: minimal-os

config:
  os:
    hostname: __IMAGE_NAME__
    packages:
      install:
        - openssh-server
        # ${efiArch} is a parameter set per-arch in by-arch/<arch>.yaml (x64 / aa64).
        - \"grub2-efi-${efiArch}\"
";

const BY_VARIANT_MINIMAL: &str = "\
# Applies to cells where variant=minimal. Deltas here merge on top of the shared config in image.yaml.
config:
  os:
    packages:
      install:
        - vim-minimal
";

const BY_VARIANT_FULL: &str = "\
# Applies to cells where variant=full.
config:
  os:
    packages:
      install:
        - vim
        - git
";

const BY_ARCH_AMD64: &str = "\
# Applies to cells where arch=amd64. `params:` are scalars you can interpolate into config with
# ${...} (here ${efiArch}, referenced by image.yaml's package list).
params:
  efiArch: x64
";

const BY_ARCH_ARM64: &str = "\
# Applies to cells where arch=arm64.
params:
  efiArch: aa64
";
