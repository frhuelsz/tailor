//! Build orchestration: render each image's cells, resolve their inputs, compute fingerprints,
//! consult build stamps, and produce a `BuildPlan`; then drive execution through the `Executor`
//! port (`meta/docs/architecture.md` §3.2, stages 11–18 of §5).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use tailor_config::{
    Access, Arch, BaseSource, OutputFormat, ToolConfig, ToolchainEntry, ToolchainRef,
    ToolsDirSourceInline, ToolsDirSourceRef, cell_slug, render_image,
};
use tokio_util::sync::CancellationToken;

use crate::{
    domain::{BuildPlan, Cell, CellSlug, CellToolsDir, PlannedCell, Target},
    error::CoreError,
    fingerprint::{FingerprintInputs, fingerprint},
    lockfile::Lockfile,
    ports::{
        BaseResolver, ExecutionContext, ExecutionResult, Executor, ResolvedBase, RuntimeConfig,
        Signer, ToolsDirPlan,
    },
    selector::Selector,
    stamp,
};

const DEFAULT_HOST_ROOT: &str = "/host";
const TOOLS_DIRS_CACHE: &str = "tools-dirs";
const TOOLS_DIR_SCRATCH: &str = "tools-dir";
const LATEST_TAG: &str = "latest";
const PREVIEW_FEATURES_KEY: &str = "previewFeatures";
const TOOLS_DIR_PREVIEW: &str = "tools-dir";

/// Options controlling a build run.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Rebuild even cells the stamp says are up to date.
    pub force: bool,
    /// Print the resolved IC invocation(s) without running.
    pub dry_run: bool,
    /// `Some(i)` for the i-th clone under `build --clones N`.
    pub clone_index: Option<u32>,
}

/// A user-facing build progress event ([`Orchestrator::build`] emits these so the CLI can report
/// progress cargo-style).
#[derive(Debug)]
pub enum BuildProgress<'a> {
    /// A cell is about to be customized — emitted before the (slow) IC run starts.
    Building { slug: &'a str },
    /// A cell finished; its artifact is at `artifact`.
    Built {
        slug: &'a str,
        artifact: &'a Path,
        exit_code: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedToolchain {
    pub ic_image_ref: String,
    pub pull: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedToolsDirSource {
    pub image_ref: String,
    pub digest: String,
    pub pull: bool,
}

/// Wires concrete adapters (a resolver and an executor) into the build pipeline.
pub struct Orchestrator<E, R> {
    executor: E,
    resolver: R,
}

impl<E: Executor, R: BaseResolver> Orchestrator<E, R> {
    pub fn new(executor: E, resolver: R) -> Self {
        Self { executor, resolver }
    }

    /// Plan a build: render and resolve every selected cell of every target, fingerprint it, and
    /// decide whether it is up to date against its build stamp.
    #[allow(
        clippy::too_many_arguments,
        reason = "planning needs config, lock, pre-resolved images, selection, and output root"
    )]
    pub async fn plan(
        &self,
        targets: &[Arc<Target>],
        tool: &ToolConfig,
        lock: &Lockfile,
        toolchains: &BTreeMap<String, ResolvedToolchain>,
        tools_dir_sources: &BTreeMap<String, ResolvedToolsDirSource>,
        selector: &Selector,
        output_dir: &Path,
    ) -> Result<BuildPlan, CoreError> {
        let mut planned = Vec::new();
        for target in targets {
            let (toolchain_id, toolchain) = toolchain_for(target, tool)?;
            let resolved_toolchain = resolved_toolchain(toolchains, &toolchain_id, &toolchain)?;
            for cell in cells_selected(target, selector)? {
                let resolved = self
                    .resolver
                    .resolve(&cell.base, cell.arch, &cell.target.dir)
                    .await?;
                let resolved_tools_dir = resolved_tools_dir(tools_dir_sources, &cell)?;
                let print = fingerprint(&FingerprintInputs {
                    slug: cell.slug.as_ref(),
                    toolchain_digest: &resolved_toolchain.ic_image_ref,
                    base: &resolved,
                    ic_config: &cell.ic_config,
                    operation: target.definition.operation.unwrap_or_default(),
                    inject_files: target.definition.inject_files.unwrap_or(false),
                    tools_dir_digest: resolved_tools_dir.map(|source| source.digest.as_str()),
                    extra_dependency_hashes: &[],
                    rpm_source_hashes: &[],
                });
                let artifact =
                    output_dir.join(artifact_name(cell.slug.as_ref(), cell.output.format));
                let up_to_date =
                    stamp::is_up_to_date(output_dir, cell.slug.as_ref(), print, &artifact);
                let runtime = runtime_config(tool, lock, &target.root);
                let tools_dir_plan = tools_dir_plan_for(&cell, resolved_tools_dir, &runtime)?;
                planned.push(PlannedCell {
                    cell,
                    fingerprint: print,
                    up_to_date,
                    base_ref: base_image_ref(&resolved),
                    tools_dir: tools_dir_plan,
                });
            }
        }
        Ok(BuildPlan { cells: planned })
    }

    /// Execute the stale cells of a plan, writing a build stamp after each success. `progress`
    /// receives a [`BuildProgress`] event before and after each cell, for user-facing reporting.
    #[allow(
        clippy::too_many_arguments,
        reason = "each parameter is a distinct, necessary build input"
    )]
    pub async fn build(
        &self,
        plan: &BuildPlan,
        tool: &ToolConfig,
        lock: &Lockfile,
        toolchains: &BTreeMap<String, ResolvedToolchain>,
        workspace_root: &Path,
        output_dir: &Path,
        options: &BuildOptions,
        cancel: CancellationToken,
        progress: &mut dyn FnMut(BuildProgress<'_>),
        signer_for: &dyn Fn(&Cell) -> Option<Arc<dyn Signer>>,
    ) -> Result<Vec<ExecutionResult>, CoreError> {
        let runtime = runtime_config(tool, lock, workspace_root);
        let mut results = Vec::new();
        for planned in &plan.cells {
            if planned.up_to_date && !options.force {
                continue;
            }
            let (toolchain_id, toolchain) = toolchain_for(&planned.cell.target, tool)?;
            let resolved_toolchain = resolved_toolchain(toolchains, &toolchain_id, &toolchain)?;
            let context = ExecutionContext {
                output_dir: output_dir.to_path_buf(),
                ic_image_ref: resolved_toolchain.ic_image_ref.clone(),
                base_ref: planned.base_ref.clone(),
                tools_dir: planned.tools_dir.clone(),
                platform: format!("linux/{}", planned.cell.arch),
                clone_index: options.clone_index,
                dry_run: options.dry_run,
                pull: resolved_toolchain.pull,
                signer: signer_for(&planned.cell),
                runtime: runtime.clone(),
            };
            progress(BuildProgress::Building {
                slug: planned.cell.slug.as_ref(),
            });
            let result = self
                .executor
                .execute(&planned.cell, &context, cancel.clone())
                .await?;
            if !options.dry_run {
                stamp::write(output_dir, planned.cell.slug.as_ref(), planned.fingerprint)?;
            }
            progress(BuildProgress::Built {
                slug: planned.cell.slug.as_ref(),
                artifact: &result.artifact_path,
                exit_code: result.exit_code,
            });
            results.push(result);
        }
        Ok(results)
    }

    /// Render and print the IC invocation for every cell **without** resolving digests or touching
    /// the network — the offline `--dry-run`/debug path (`meta/docs/design.md` §11). Uses the toolchain
    /// tag (not a digest) for the container reference and delegates to the executor, which
    /// short-circuits before any pull/run when `dry_run` is set.
    pub async fn dry_run(
        &self,
        targets: &[Arc<Target>],
        tool: &ToolConfig,
        selector: &Selector,
        workspace_root: &Path,
        output_dir: &Path,
        signer_for: &dyn Fn(&Cell) -> Option<Arc<dyn Signer>>,
    ) -> Result<Vec<ExecutionResult>, CoreError> {
        let runtime = runtime_config(tool, &Lockfile::default(), workspace_root);
        let mut results = Vec::new();
        for target in targets {
            let (_, toolchain) = toolchain_for(target, tool)?;
            let ic_image_ref = format!("{}:{}", toolchain.container, toolchain.effective_tag());
            for cell in cells_selected(target, selector)? {
                let context = ExecutionContext {
                    output_dir: output_dir.to_path_buf(),
                    ic_image_ref: ic_image_ref.clone(),
                    // `--dry-run` resolves no digests, so the executor falls back to the un-pinned
                    // base reference for the preview.
                    base_ref: None,
                    tools_dir: tools_dir_plan_for(&cell, None, &runtime)?,
                    platform: format!("linux/{}", cell.arch),
                    clone_index: None,
                    dry_run: true,
                    pull: false,
                    signer: signer_for(&cell),
                    runtime: runtime.clone(),
                };
                results.push(
                    self.executor
                        .execute(&cell, &context, CancellationToken::new())
                        .await?,
                );
            }
        }
        Ok(results)
    }
}

/// Resolve a target's toolchain to its `(name, entry)`, applying the workspace default if unset.
pub fn toolchain_for(
    target: &Target,
    tool: &ToolConfig,
) -> Result<(String, ToolchainEntry), CoreError> {
    let lookup = |id: &str| -> Result<ToolchainEntry, CoreError> {
        tool.toolchains
            .get(id)
            .cloned()
            .ok_or_else(|| CoreError::UnknownToolchain {
                id: id.to_owned(),
                image: target.name().to_owned(),
            })
    };
    match &target.definition.toolchain {
        Some(ToolchainRef::Inline(entry)) => Ok(("inline".to_owned(), entry.clone())),
        Some(ToolchainRef::Id(id)) => Ok((id.clone(), lookup(id)?)),
        None => {
            let id = &tool.toolchains.default;
            Ok((id.clone(), lookup(id)?))
        }
    }
}

pub fn toolchain_key(entry: &ToolchainEntry) -> String {
    if has_tag_or_digest(&entry.container) {
        return entry.container.clone();
    }
    format!("{}:{}", entry.container, entry.effective_tag())
}

pub fn tools_dir_key(source: &ToolsDirSourceInline) -> String {
    if has_tag_or_digest(&source.container) {
        return source.container.clone();
    }
    format!("{}:{}", source.container, source.effective_tag())
}

fn resolved_toolchain<'a>(
    toolchains: &'a BTreeMap<String, ResolvedToolchain>,
    id: &str,
    entry: &ToolchainEntry,
) -> Result<&'a ResolvedToolchain, CoreError> {
    let key = toolchain_key(entry);
    toolchains
        .get(&key)
        .ok_or_else(|| CoreError::MissingResolvedToolchain {
            id: id.to_owned(),
            reference: key,
        })
}

fn resolved_tools_dir<'a>(
    sources: &'a BTreeMap<String, ResolvedToolsDirSource>,
    cell: &Cell,
) -> Result<Option<&'a ResolvedToolsDirSource>, CoreError> {
    let Some(tools_dir) = &cell.tools_dir else {
        return Ok(None);
    };
    let key = tools_dir_key(&tools_dir.source);
    sources
        .get(&key)
        .map(Some)
        .ok_or(CoreError::MissingResolvedToolsDir { reference: key })
}

fn resolve_tools_dir(target: &Target) -> Result<Option<CellToolsDir>, CoreError> {
    let Some(tools_dir) = &target.definition.tools_dir else {
        return Ok(None);
    };
    let (source, source_name) = match &tools_dir.source {
        ToolsDirSourceRef::Inline(source) => (source.clone(), None),
        ToolsDirSourceRef::Id(name) => {
            let source = target
                .tools_dir_sources
                .iter()
                .find(|source| &source.name == name)
                .ok_or_else(|| CoreError::UnknownToolsDirSource {
                    image: target.name().to_owned(),
                    name: name.clone(),
                    known: target
                        .tools_dir_sources
                        .iter()
                        .map(|source| source.name.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                })?;
            (source.inline(), Some(name.clone()))
        }
    };
    Ok(Some(CellToolsDir {
        source,
        source_name,
        access: tools_dir.access,
    }))
}

fn validate_tools_dir_preview(target: &Target) -> Result<(), CoreError> {
    if target.definition.tools_dir.is_none() {
        return Ok(());
    }
    let Some(config) = &target.definition.config else {
        return Ok(());
    };
    if !matches!(config, serde_yaml_ng::Value::Mapping(_)) {
        return Ok(());
    }
    if tools_dir_preview_enabled(config) {
        Ok(())
    } else {
        Err(CoreError::ToolsDirPreviewMissing {
            image: target.name().to_owned(),
        })
    }
}

fn tools_dir_preview_enabled(config: &serde_yaml_ng::Value) -> bool {
    config
        .get(PREVIEW_FEATURES_KEY)
        .and_then(serde_yaml_ng::Value::as_sequence)
        .is_some_and(|features| {
            features
                .iter()
                .any(|feature| feature.as_str() == Some(TOOLS_DIR_PREVIEW))
        })
}

fn tools_dir_plan_for(
    cell: &Cell,
    resolved: Option<&ResolvedToolsDirSource>,
    runtime: &RuntimeConfig,
) -> Result<Option<ToolsDirPlan>, CoreError> {
    let Some(tools_dir) = &cell.tools_dir else {
        return Ok(None);
    };
    let resolved_digest = resolved.map(|source| source.digest.as_str());
    let cache_key = resolved_digest.map_or_else(
        || unpinned_tools_dir_key(&tools_dir.source),
        sanitize_digest,
    );
    let cache_root = runtime.image_cache_dir.clone().unwrap_or_else(|| {
        runtime
            .workspace_root
            .join(tailor_config::defaults::DEFAULT_IMAGE_CACHE_DIR)
    });
    let cache_dir = cache_root.join(TOOLS_DIRS_CACHE).join(&cache_key);
    if cache_dir == Path::new("/") {
        return Err(CoreError::Exec(crate::error::ExecError::UnsafeDir {
            path: cache_dir,
            reason: "tools-dir cache must not be filesystem root".to_owned(),
        }));
    }
    let mount_dir = match tools_dir.access {
        Access::Ro => cache_dir.clone(),
        Access::Rw => {
            let Some(build_base) = &runtime.build_dir_base else {
                return Err(CoreError::WritableToolsDirNeedsBuildDir {
                    image: cell.target.name().to_owned(),
                });
            };
            build_base.join(cell.slug.as_ref()).join(TOOLS_DIR_SCRATCH)
        }
    };
    Ok(Some(ToolsDirPlan {
        image_ref: resolved.map_or_else(
            || tools_dir_key(&tools_dir.source),
            |source| source.image_ref.clone(),
        ),
        digest: resolved_digest.map_or_else(|| cache_key.clone(), str::to_owned),
        pull: resolved.is_some_and(|source| source.pull),
        cache_dir,
        mount_dir,
        access: tools_dir.access,
    }))
}

fn sanitize_digest(digest: &str) -> String {
    digest.replace([':', '/'], "_")
}

fn unpinned_tools_dir_key(source: &ToolsDirSourceInline) -> String {
    sanitize_digest(&format!(
        "{}:{}",
        source.container,
        source.tag.as_deref().unwrap_or(LATEST_TAG)
    ))
}

fn has_tag_or_digest(reference: &str) -> bool {
    reference.contains('@')
        || reference
            .rsplit('/')
            .next()
            .is_some_and(|segment| segment.contains(':'))
}

/// Expand a target into its cells: one per (rendered matrix point × architecture × output format).
pub fn cells(target: &Arc<Target>) -> Result<Vec<Cell>, CoreError> {
    validate_tools_dir_preview(target)?;
    let rendered = render_image(&target.definition, &target.dir)?;
    let mut cells = Vec::new();
    for rc in rendered {
        // A `base: { ref: <name> }` reference resolves to its catalogue slot's file (workspace-root
        // relative) and behaves like a `path` base; the slot's `arch` reconciles below (§3).
        let (base, base_image, slot_arch) = resolve_base(target, &rc.base)?;
        let arch_is_axis = rc.tuple.get("arch").is_some();
        let arches = match rc.tuple.get("arch") {
            Some(value) => vec![parse_arch(value).ok_or_else(|| CoreError::MissingArchBase {
                image: target.name().to_owned(),
                arch: value.to_owned(),
            })?],
            // No `arch` axis: the base's own arch (a catalogue slot's `arch`, a local `path` base's
            // `arch`, or an `oci.platform`'s arch component) supplies it, else the `amd64` default.
            None => vec![slot_arch.unwrap_or(Arch::Amd64)],
        };
        for arch in arches {
            check_platform_arch(target, &rc.base, arch)?;
            reconcile_slot_arch(target, base_image.as_deref(), slot_arch, arch)?;
            let mut axes: BTreeMap<String, String> = rc.tuple.values.iter().cloned().collect();
            axes.entry("arch".to_owned())
                .or_insert_with(|| arch.as_str().to_owned());
            let outputs = if rc.outputs.is_empty() {
                &target.default_outputs
            } else {
                &rc.outputs
            };
            for output in outputs {
                let slug = slug_for(target.name(), &rc, arch, arch_is_axis, output.format);
                let tools_dir = resolve_tools_dir(target)?;
                cells.push(Cell {
                    target: Arc::clone(target),
                    axes: axes.clone(),
                    arch,
                    output: output.clone(),
                    slug: CellSlug(slug),
                    ic_config: rc.ic_config.clone(),
                    base: base.clone(),
                    base_image: base_image.clone(),
                    rpm_sources: rc.rpm_sources.clone(),
                    tools_dir,
                });
            }
        }
    }
    Ok(cells)
}

/// Resolve a `base: { ref: <name> }` reference against the workspace catalogue. Returns the
/// effective base (a catalogue slot collapses to a workspace-root-relative `path` base), the slot
/// name for the matrix output, and the slot's declared `arch`. An unknown name is a config error
/// surfaced by `validate` (`meta/docs/base-image-catalogue.md` §6). `path`/`oci`/`azureLinux` pass
/// through unchanged.
fn resolve_base(
    target: &Target,
    base: &BaseSource,
) -> Result<(BaseSource, Option<String>, Option<Arch>), CoreError> {
    match base {
        BaseSource::Ref { reference } => {
            let slot =
                target
                    .base_images
                    .get(reference)
                    .ok_or_else(|| CoreError::UnknownBaseImage {
                        image: target.name().to_owned(),
                        name: reference.clone(),
                        known: target
                            .base_images
                            .iter()
                            .map(|slot| slot.name.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                    })?;
            let path = tailor_config::absolutize(&slot.path, &target.root);
            Ok((
                BaseSource::Path {
                    path,
                    arch: slot.arch,
                },
                Some(reference.clone()),
                slot.arch,
            ))
        }
        // A local `path` base's own `arch` supplies the cell arch when no `arch` axis is declared.
        BaseSource::Path { arch, .. } => Ok((base.clone(), None, *arch)),
        // A multi-arch registry base pins its arch via `oci.platform`'s arch component.
        BaseSource::Oci { oci } => Ok((
            base.clone(),
            None,
            oci.platform.as_deref().and_then(platform_arch),
        )),
        // `azureLinux` declares no arch, so the cell arch (axis or default) decides.
        BaseSource::AzureLinux { .. } => Ok((base.clone(), None, None)),
    }
}

/// Reconcile a catalogue slot's `arch` against the cell's effective arch — they must agree
/// (`meta/docs/arch-and-platform.md` §3). A slot with no `arch` never conflicts. Offline, so
/// `validate` catches a mismatch before any file is read.
fn reconcile_slot_arch(
    target: &Target,
    base_image: Option<&str>,
    slot_arch: Option<Arch>,
    arch: Arch,
) -> Result<(), CoreError> {
    match (base_image, slot_arch) {
        (Some(name), Some(slot)) if slot != arch => Err(CoreError::BaseImageArchMismatch {
            image: target.name().to_owned(),
            slug: format!("{}_{}", target.name(), arch),
            name: name.to_owned(),
            slot_arch: slot,
            cell_arch: arch,
        }),
        _ => Ok(()),
    }
}

/// Reconcile a cell's effective arch against the base image: an `oci.platform`'s arch component must
/// equal the cell arch. `path`/`azureLinux` declare no arch, so they never conflict. Host-independent
/// and offline, so `validate` catches a mismatch before any pull (`meta/docs/arch-and-platform.md` §3).
fn check_platform_arch(target: &Target, base: &BaseSource, arch: Arch) -> Result<(), CoreError> {
    let BaseSource::Oci { oci } = base else {
        return Ok(());
    };
    let Some(platform) = oci.platform.as_deref() else {
        return Ok(());
    };
    match platform_arch(platform) {
        Some(platform_arch) if platform_arch == arch => Ok(()),
        Some(platform_arch) => Err(CoreError::PlatformArchMismatch {
            image: target.name().to_owned(),
            slug: format!("{}_{}", target.name(), arch),
            platform: platform.to_owned(),
            platform_arch: platform_arch.as_str().to_owned(),
            cell_arch: arch,
        }),
        None => Ok(()),
    }
}

/// The arch component of a `linux/<arch>[/<variant>]` platform, if it is a known [`Arch`].
fn platform_arch(platform: &str) -> Option<Arch> {
    platform.split('/').nth(1).and_then(parse_arch)
}

/// Expand a target's cells (as [`cells`]) and narrow them with `selector`, validating that every
/// constrained axis is declared and that the selection is non-empty (catches typos in CI matrices).
pub fn cells_selected(target: &Arc<Target>, selector: &Selector) -> Result<Vec<Cell>, CoreError> {
    let all = cells(target)?;
    if selector.is_empty() {
        return Ok(all);
    }
    let declared: BTreeSet<&str> = all
        .iter()
        .flat_map(|cell| cell.axes.keys().map(String::as_str))
        .collect();
    for axis in selector.axis_names() {
        if !declared.contains(axis) {
            let mut names: Vec<&str> = declared.iter().copied().collect();
            names.sort_unstable();
            return Err(CoreError::UnknownSelectorAxis {
                axis: axis.to_owned(),
                image: target.name().to_owned(),
                declared: names.join(", "),
            });
        }
    }
    let selected: Vec<Cell> = all
        .into_iter()
        .filter(|cell| selector.matches(cell))
        .collect();
    if selected.is_empty() {
        return Err(CoreError::NoCellsSelected {
            image: target.name().to_owned(),
        });
    }
    Ok(selected)
}

fn slug_for(
    name: &str,
    rc: &tailor_config::RenderedCell,
    arch: Arch,
    arch_is_axis: bool,
    format: OutputFormat,
) -> String {
    if arch_is_axis {
        return cell_slug(name, &rc.tuple, format);
    }
    let coordinate = rc.tuple.coordinate();
    if coordinate.is_empty() {
        format!("{name}_{arch}_{format}")
    } else {
        format!("{name}_{coordinate}_{arch}_{format}")
    }
}

/// Build the execution runtime config from the tool config, workspace root, and lock (janitor digest).
pub fn runtime_config(tool: &ToolConfig, lock: &Lockfile, workspace_root: &Path) -> RuntimeConfig {
    let runtime = tool.runtime.as_ref();
    let mounts = runtime.and_then(|r| r.mounts.as_ref());
    let host_root = mounts
        .and_then(|m| m.host_root.clone())
        .unwrap_or_else(|| DEFAULT_HOST_ROOT.into());
    let extra_paths = mounts.map_or_else(Vec::new, |mounts| {
        mounts
            .extra_paths
            .iter()
            .map(|mount| tailor_config::ExtraMount {
                path: tailor_config::absolutize(&mount.path, workspace_root),
                access: mount.access,
            })
            .collect()
    });
    let janitor = runtime.and_then(|r| r.janitor_image.as_ref());
    let janitor_image = janitor.map_or_else(
        || {
            // No janitor configured: fall back to the built-in default so root-owned IC outputs are
            // still normalized sudo-free out of the box (`meta/docs/design.md` §7.7).
            format!(
                "{}:{}",
                tailor_config::defaults::DEFAULT_JANITOR_CONTAINER,
                tailor_config::defaults::DEFAULT_JANITOR_TAG
            )
        },
        |j| {
            let digest = lock
                .runtime
                .as_ref()
                .and_then(|r| r.janitor_image.as_ref())
                .map(|c| c.digest.as_str());
            match digest {
                Some(digest) => format!("{}@{digest}", j.container),
                None => format!("{}:{}", j.container, j.tag.as_deref().unwrap_or("latest")),
            }
        },
    );
    RuntimeConfig {
        host_root,
        workspace_root: workspace_root.to_path_buf(),
        privileged: runtime.and_then(|r| r.privileged).unwrap_or(true),
        mount_dev: mounts.and_then(|m| m.dev).unwrap_or(true),
        build_dir_base: runtime
            .and_then(|r| r.build_dir_base.clone())
            .map(|path| tailor_config::absolutize(path, workspace_root)),
        log_level: runtime.and_then(|r| r.log_level.map(|l| l.as_str().to_owned())),
        image_cache_dir: runtime.and_then(|r| r.image_cache_dir.clone()),
        log_dir: runtime.and_then(|r| r.log_dir.clone()),
        extra_paths,
        janitor_image,
    }
}

fn parse_arch(value: &str) -> Option<Arch> {
    match value {
        "amd64" => Some(Arch::Amd64),
        "arm64" => Some(Arch::Arm64),
        _ => None,
    }
}

/// The digest-pinned IC `--image` reference for a resolved base — `Some("oci:<repo>@<digest>")` for
/// a registry base, `None` for a local file (which uses `--image-file`). Pinning the digest keeps
/// registry builds reproducible (`meta/docs/design.md` §5.2/§6).
fn base_image_ref(resolved: &ResolvedBase) -> Option<String> {
    match resolved {
        ResolvedBase::Oci {
            reference, digest, ..
        } => Some(format!("oci:{}@{digest}", oci_repository(reference))),
        ResolvedBase::LocalFile { .. } => None,
    }
}

/// The bare repository of an OCI reference — strips a `@digest` and/or a `:tag` so a resolved digest
/// can be reattached. The tag is the `:` *after* the last `/` (so a `host:port/repo` registry port
/// is preserved).
fn oci_repository(reference: &str) -> &str {
    let without_digest = reference.split('@').next().unwrap_or(reference);
    match without_digest.rfind('/') {
        Some(slash) => match without_digest[slash..].find(':') {
            Some(colon) => &without_digest[..slash + colon],
            None => without_digest,
        },
        None => without_digest.split(':').next().unwrap_or(without_digest),
    }
}

/// The artifact filename for a cell slug + format (a directory for `pxe-dir`).
pub fn artifact_name(slug: &str, format: OutputFormat) -> String {
    let extension = match format {
        OutputFormat::Cosi => "cosi",
        OutputFormat::Vhd | OutputFormat::VhdFixed => "vhd",
        OutputFormat::Vhdx => "vhdx",
        OutputFormat::Qcow2 => "qcow2",
        OutputFormat::Raw | OutputFormat::BaremetalImage => "raw",
        OutputFormat::Iso => "iso",
        OutputFormat::PxeTar => "tar.gz",
        OutputFormat::PxeDir => return slug.to_owned(),
    };
    format!("{slug}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;
    use tailor_config::{
        BaseImageCatalogue, BaseImageSlot, OutputArtifactsPolicy, OutputSpec, load_image,
    };
    use tempfile::TempDir;

    use crate::testing::{FakeExecutor, FakeResolver};

    /// Write `body` to `<root>/<rel>`, creating parent directories as needed.
    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// A small two-axis matrix target (edition[2] × arch[2] = 4 cells) backed by a tempdir. The
    /// returned `TempDir` must be kept alive for as long as the target is used (renders read it).
    fn mini_target() -> (TempDir, Arc<Target>) {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "image.yaml",
            indoc! {"
                name: mini
                matrix:
                  edition: [lite, pro]
                  arch: [amd64, arm64]
                outputs:
                  - format: cosi
                base:
                  path: ./b.img
                config:
                  os:
                    hostname: mini
            "},
        );
        let definition = load_image(tmp.path().join("image.yaml")).unwrap();
        let target = Arc::new(Target {
            definition,
            dir: tmp.path().to_path_buf(),
            default_outputs: Vec::new(),
            output_artifacts: OutputArtifactsPolicy::default(),
            root: tmp.path().to_path_buf(),
            base_images: BaseImageCatalogue::default(),
            tools_dir_sources: Vec::new(),
        });
        (tmp, target)
    }

    fn tool_config() -> ToolConfig {
        serde_yaml_ng::from_str(indoc! {"
            schemaVersion: 1
            toolchains:
              default: ic
              entries:
                - name: ic
                  container: registry.example/imagecustomizer
                  version: 1.0.0
        "})
        .unwrap()
    }

    fn resolved_toolchains(tool: &ToolConfig) -> BTreeMap<String, ResolvedToolchain> {
        let entry = tool.toolchains.get(&tool.toolchains.default).unwrap();
        BTreeMap::from([(
            toolchain_key(entry),
            ResolvedToolchain {
                ic_image_ref: "registry.example/imagecustomizer@sha256:faketoolchain".to_owned(),
                pull: true,
            },
        )])
    }

    fn resolved_tools_dir_sources() -> BTreeMap<String, ResolvedToolsDirSource> {
        BTreeMap::from([(
            "registry.example/tools:latest".to_owned(),
            ResolvedToolsDirSource {
                image_ref: "registry.example/tools@sha256:faketoolsdir".to_owned(),
                digest: "sha256:faketoolsdir".to_owned(),
                pull: true,
            },
        )])
    }

    #[tokio::test]
    async fn plans_all_cells_as_stale() {
        let orchestrator = Orchestrator::new(FakeExecutor::default(), FakeResolver);
        let (_tmp, target) = mini_target();
        let tool = tool_config();
        let toolchains = resolved_toolchains(&tool);
        let lock = Lockfile::default();
        let out = TempDir::new().unwrap();

        let plan = orchestrator
            .plan(
                &[target],
                &tool,
                &lock,
                &toolchains,
                &BTreeMap::new(),
                &Selector::default(),
                out.path(),
            )
            .await
            .unwrap();

        // edition[2] × arch[2] = 4 cells, one output format each.
        assert_eq!(plan.cells.len(), 4);
        assert!(plan.cells.iter().all(|c| !c.up_to_date));
        assert_eq!(plan.stale().count(), 4);
    }

    #[tokio::test]
    async fn build_executes_stale_cells_and_writes_stamps() {
        let executor = FakeExecutor::default();
        let recorder = executor.recorder();
        let orchestrator = Orchestrator::new(executor, FakeResolver);
        let (_tmp, target) = mini_target();
        let tool = tool_config();
        let toolchains = resolved_toolchains(&tool);
        let lock = Lockfile::default();
        let out = TempDir::new().unwrap();

        let plan = orchestrator
            .plan(
                &[target],
                &tool,
                &lock,
                &toolchains,
                &BTreeMap::new(),
                &Selector::default(),
                out.path(),
            )
            .await
            .unwrap();
        let results = orchestrator
            .build(
                &plan,
                &tool,
                &lock,
                &toolchains,
                out.path(),
                out.path(),
                &BuildOptions::default(),
                CancellationToken::new(),
                &mut |_| {},
                &|_| None,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 4);
        assert_eq!(
            recorder.lock().unwrap().len(),
            4,
            "executor invoked once per stale cell"
        );
        // A second plan now sees the stamps; but the fake executor wrote no artifacts, so the
        // up-to-date check fails and every cell stays stale.
        let (_tmp2, target2) = mini_target();
        let replan = orchestrator
            .plan(
                &[target2],
                &tool,
                &lock,
                &toolchains,
                &BTreeMap::new(),
                &Selector::default(),
                out.path(),
            )
            .await
            .unwrap();
        assert_eq!(replan.cells.len(), 4);
    }

    #[tokio::test]
    async fn dry_run_renders_every_cell_without_resolution() {
        // dry_run never calls the resolver (no digests needed), so it works fully offline.
        let executor = FakeExecutor::default();
        let recorder = executor.recorder();
        let orchestrator = Orchestrator::new(executor, FakeResolver);
        let (_tmp, target) = mini_target();
        let out = TempDir::new().unwrap();

        let results = orchestrator
            .dry_run(
                &[target],
                &tool_config(),
                &Selector::default(),
                out.path(),
                out.path(),
                &|_| None,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 4);
        assert_eq!(recorder.lock().unwrap().len(), 4);
    }

    #[test]
    fn cells_selected_narrows_to_a_slice_and_a_single_cell() {
        let (_tmp, target) = mini_target();
        // A one-axis slice: every amd64 cell (edition[2]).
        let slice = Selector::parse(&["arch=amd64".to_owned()], &[], &[]).unwrap();
        assert_eq!(cells_selected(&target, &slice).unwrap().len(), 2);

        // Pinning every axis yields exactly one cell.
        let one = Selector::parse(&["edition=lite,arch=amd64".to_owned()], &[], &[]).unwrap();
        let selected = cells_selected(&target, &one).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].slug.as_ref(), "mini_lite_amd64_cosi");
    }

    #[test]
    fn cells_selected_rejects_unknown_axis_and_empty_selection() {
        let (_tmp, target) = mini_target();
        let bad_axis = Selector::parse(&["distro=fedora".to_owned()], &[], &[]).unwrap();
        assert!(matches!(
            cells_selected(&target, &bad_axis).unwrap_err(),
            CoreError::UnknownSelectorAxis { .. }
        ));
        let no_match = Selector::parse(&["edition=does-not-exist".to_owned()], &[], &[]).unwrap();
        assert!(matches!(
            cells_selected(&target, &no_match).unwrap_err(),
            CoreError::NoCellsSelected { .. }
        ));
    }

    /// Build a single-cell target from `body`. With no `arch` axis and no base arch, a cell resolves
    /// to the built-in `amd64` default; a base's own `arch` (slot/path/oci) supplies it otherwise.
    fn target_with(body: &str) -> (TempDir, Arc<Target>) {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "image.yaml", body);
        let definition = load_image(tmp.path().join("image.yaml")).unwrap();
        let target = Arc::new(Target {
            definition,
            dir: tmp.path().to_path_buf(),
            default_outputs: vec![OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: None,
                name: None,
            }],
            output_artifacts: OutputArtifactsPolicy::default(),
            root: tmp.path().to_path_buf(),
            base_images: BaseImageCatalogue::default(),
            tools_dir_sources: Vec::new(),
        });
        (tmp, target)
    }

    const NO_ARCH_IMAGE: &str = indoc! {"
        name: solo
        base:
          path: ./b.img
        config:
          os:
            hostname: solo
    "};

    #[test]
    fn defaults_to_amd64_with_no_arch_axis_and_no_architectures() {
        let (_tmp, target) = target_with(NO_ARCH_IMAGE);
        let cells = cells(&target).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].arch, Arch::Amd64);
        assert_eq!(cells[0].slug.as_ref(), "solo_amd64_cosi");
    }

    #[test]
    fn path_base_arch_supplies_cell_arch_when_image_declares_none() {
        let (_tmp, target) = target_with(indoc! {"
            name: solo
            base:
              path: ./b.img
              arch: arm64
            config:
              os:
                hostname: solo
        "});
        let cells = cells(&target).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].arch, Arch::Arm64);
    }

    #[test]
    fn arch_axis_drives_one_cell_per_value() {
        let (_tmp, target) = target_with(indoc! {"
                name: solo
                matrix:
                  arch: [amd64, arm64]
                base:
                  path: ./b.img
                config:
                  os:
                    hostname: solo
            "});
        let mut arches: Vec<Arch> = cells(&target).unwrap().iter().map(|c| c.arch).collect();
        arches.sort_unstable();
        assert_eq!(arches, vec![Arch::Amd64, Arch::Arm64]);
    }

    #[test]
    fn oci_platform_mismatch_is_an_error() {
        // Image pinned amd64 (axis) but the base pins arm64 → reconcile error (offline, no pull).
        let (_tmp, target) = target_with(indoc! {"
                name: solo
                matrix:
                  arch: [amd64]
                base:
                  oci:
                    uri: registry.example/base:edge
                    platform: linux/arm64
                config:
                  os:
                    hostname: solo
            "});
        assert!(matches!(
            cells(&target).unwrap_err(),
            CoreError::PlatformArchMismatch { .. }
        ));
    }

    #[test]
    fn oci_platform_supplies_cell_arch_when_no_axis() {
        // No arch axis: a fixed `oci.platform` supplies the cell arch (base arch fills in).
        let (_tmp, target) = target_with(indoc! {"
                name: solo
                base:
                  oci:
                    uri: registry.example/base:edge
                    platform: linux/arm64
                config:
                  os:
                    hostname: solo
            "});
        assert_eq!(cells(&target).unwrap()[0].arch, Arch::Arm64);
    }

    #[test]
    fn oci_platform_matching_arch_is_accepted() {
        let (_tmp, target) = target_with(indoc! {"
                name: solo
                base:
                  oci:
                    uri: registry.example/base:edge
                    platform: linux/amd64
                config:
                  os:
                    hostname: solo
            "});
        assert_eq!(cells(&target).unwrap()[0].arch, Arch::Amd64);
    }

    #[test]
    fn oci_platform_matches_declared_arch_axis() {
        let (_tmp, target) = target_with(indoc! {"
                name: solo
                matrix:
                  arch: [arm64]
                base:
                  oci:
                    uri: registry.example/base:edge
                    platform: linux/${arch}
                config:
                  os:
                    hostname: solo
            "});
        assert_eq!(cells(&target).unwrap()[0].arch, Arch::Arm64);
    }

    /// Single-cell target plus a `baseImages:` catalogue, for `base: { ref: <name> }` resolution and
    /// §3 arch reconciliation (the slot's `arch` supplies the cell arch when no axis is declared).
    fn target_with_catalogue(body: &str, catalogue: BaseImageCatalogue) -> (TempDir, Arc<Target>) {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "image.yaml", body);
        let definition = load_image(tmp.path().join("image.yaml")).unwrap();
        let target = Arc::new(Target {
            definition,
            dir: tmp.path().to_path_buf(),
            default_outputs: vec![OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: None,
                name: None,
            }],
            output_artifacts: OutputArtifactsPolicy::default(),
            root: tmp.path().to_path_buf(),
            base_images: catalogue,
            tools_dir_sources: Vec::new(),
        });
        (tmp, target)
    }

    const SLOT_IMAGE: &str = indoc! {"
        name: solo
        base:
          ref: baremetal
        config:
          os:
            hostname: solo
    "};

    fn slot(name: &str, arch: Option<Arch>) -> BaseImageSlot {
        BaseImageSlot {
            name: name.to_owned(),
            path: "bases/baremetal.vhdx".into(),
            arch,
            source: None,
        }
    }

    #[test]
    fn base_image_resolves_to_root_relative_path_and_exposes_name() {
        let cat = BaseImageCatalogue::from(vec![slot("baremetal", Some(Arch::Amd64))]);
        let (tmp, target) = target_with_catalogue(SLOT_IMAGE, cat);
        let cells = cells(&target).unwrap();
        assert_eq!(cells[0].base_image.as_deref(), Some("baremetal"));
        let BaseSource::Path { path, .. } = &cells[0].base else {
            panic!("slot did not collapse to a path base: {:?}", cells[0].base);
        };
        assert_eq!(path, &tmp.path().join("bases/baremetal.vhdx"));
    }

    #[test]
    fn slot_arch_supplies_cell_arch_when_image_declares_none() {
        let cat = BaseImageCatalogue::from(vec![slot("baremetal", Some(Arch::Arm64))]);
        let (_tmp, target) = target_with_catalogue(SLOT_IMAGE, cat);
        assert_eq!(cells(&target).unwrap()[0].arch, Arch::Arm64);
    }

    #[test]
    fn slot_arch_conflict_with_axis_is_an_error() {
        let cat = BaseImageCatalogue::from(vec![slot("baremetal", Some(Arch::Arm64))]);
        let (_tmp, target) = target_with_catalogue(
            indoc! {"
                name: solo
                matrix:
                  arch: [amd64]
                base:
                  ref: baremetal
                config:
                  os:
                    hostname: solo
            "},
            cat,
        );
        assert!(
            matches!(
                cells(&target).unwrap_err(),
                CoreError::BaseImageArchMismatch { .. }
            ),
            "expected arch mismatch"
        );
    }

    #[test]
    fn unknown_base_image_name_is_a_config_error() {
        let cat = BaseImageCatalogue::from(vec![slot("other", None)]);
        let (_tmp, target) = target_with_catalogue(SLOT_IMAGE, cat);
        assert!(
            matches!(
                cells(&target).unwrap_err(),
                CoreError::UnknownBaseImage { .. }
            ),
            "expected unknown base image"
        );
    }

    #[test]
    fn unknown_tools_dir_source_name_is_a_config_error() {
        let (_tmp, target) = target_with(indoc! {"
            name: solo
            base:
              path: ./b.img
            toolsDir:
              source: missing
            config:
              previewFeatures:
                - tools-dir
        "});
        assert!(
            matches!(
                cells(&target).unwrap_err(),
                CoreError::UnknownToolsDirSource { .. }
            ),
            "expected unknown tools-dir source"
        );
    }

    #[test]
    fn tools_dir_requires_preview_feature_when_config_is_inline() {
        let (_tmp, target) = target_with(indoc! {"
            name: solo
            base:
              path: ./b.img
            toolsDir:
              source:
                container: registry.example/tools
            config:
              os:
                hostname: solo
        "});
        assert!(
            matches!(
                cells(&target).unwrap_err(),
                CoreError::ToolsDirPreviewMissing { .. }
            ),
            "expected missing preview feature"
        );
    }

    #[tokio::test]
    async fn rw_tools_dir_requires_build_dir_base() {
        let orchestrator = Orchestrator::new(FakeExecutor::default(), FakeResolver);
        let (_tmp, target) = target_with(indoc! {"
            name: solo
            base:
              path: ./b.img
            toolsDir:
              source:
                container: registry.example/tools
              access: rw
            config:
              previewFeatures:
                - tools-dir
        "});
        let out = TempDir::new().unwrap();
        let tool = tool_config();
        let toolchains = resolved_toolchains(&tool);
        let err = orchestrator
            .plan(
                &[target],
                &tool,
                &Lockfile::default(),
                &toolchains,
                &resolved_tools_dir_sources(),
                &Selector::default(),
                out.path(),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::WritableToolsDirNeedsBuildDir { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn sourceless_slot_arch_unset_defaults_to_amd64() {
        let cat = BaseImageCatalogue::from(vec![slot("baremetal", None)]);
        let (_tmp, target) = target_with_catalogue(SLOT_IMAGE, cat);
        assert_eq!(cells(&target).unwrap()[0].arch, Arch::Amd64);
    }

    #[test]
    fn oci_repository_strips_tag_and_digest() {
        assert_eq!(
            oci_repository("mcr.microsoft.com/azurelinux/3.0/image/minimal-os:latest"),
            "mcr.microsoft.com/azurelinux/3.0/image/minimal-os"
        );
        assert_eq!(
            oci_repository("registry.example/base@sha256:abc"),
            "registry.example/base"
        );
        assert_eq!(
            oci_repository("registry.example/base:1.2@sha256:abc"),
            "registry.example/base"
        );
        // A `host:port` registry must be preserved (the tag is the `:` after the last `/`).
        assert_eq!(
            oci_repository("localhost:5000/base:dev"),
            "localhost:5000/base"
        );
        assert_eq!(oci_repository("repo"), "repo");
    }

    #[test]
    fn base_image_ref_pins_registry_bases_and_skips_local() {
        let oci = ResolvedBase::Oci {
            reference: "mcr.microsoft.com/azurelinux/3.0/image/minimal-os:latest".to_owned(),
            platform: "linux/amd64".to_owned(),
            digest: "sha256:dead".to_owned(),
        };
        assert_eq!(
            base_image_ref(&oci).as_deref(),
            Some("oci:mcr.microsoft.com/azurelinux/3.0/image/minimal-os@sha256:dead")
        );
        let local = ResolvedBase::LocalFile {
            sha256: [0; 32],
            size: 0,
        };
        assert_eq!(base_image_ref(&local), None);
    }
}
