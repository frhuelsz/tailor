//! Build orchestration: render each image's cells, resolve their inputs, compute fingerprints,
//! consult build stamps, and produce a `BuildPlan`; then drive execution through the `Executor`
//! port (`meta/docs/architecture.md` §3.2, stages 11–18 of §5).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use tailor_config::{
    Arch, BaseSource, OutputFormat, ToolConfig, ToolchainEntry, ToolchainRef, cell_slug,
    render_image,
};
use tokio_util::sync::CancellationToken;

use crate::{
    domain::{BuildPlan, Cell, CellSlug, PlannedCell, Target},
    error::CoreError,
    fingerprint::{FingerprintInputs, fingerprint},
    lockfile::Lockfile,
    ports::{
        BaseResolver, ExecutionContext, ExecutionResult, Executor, ResolvedBase, RuntimeConfig,
    },
    selector::Selector,
    stamp,
};

const DEFAULT_HOST_ROOT: &str = "/host";

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
    pub async fn plan(
        &self,
        targets: &[Arc<Target>],
        tool: &ToolConfig,
        lock: &Lockfile,
        selector: &Selector,
        output_dir: &Path,
    ) -> Result<BuildPlan, CoreError> {
        let mut planned = Vec::new();
        for target in targets {
            let (toolchain_id, toolchain) = toolchain_for(target, tool)?;
            let toolchain_digest = self
                .toolchain_digest(&toolchain_id, &toolchain, lock)
                .await?;
            for cell in cells_selected(target, selector)? {
                let resolved = self
                    .resolver
                    .resolve(&cell.base, cell.arch, &cell.target.dir)
                    .await?;
                let print = fingerprint(&FingerprintInputs {
                    slug: cell.slug.as_ref(),
                    toolchain_digest: &toolchain_digest,
                    base: &resolved,
                    ic_config: &cell.ic_config,
                    operation: target.definition.operation.unwrap_or_default(),
                    inject_files: target.definition.inject_files.unwrap_or(false),
                    extra_dependency_hashes: &[],
                    rpm_source_hashes: &[],
                });
                let artifact =
                    output_dir.join(artifact_name(cell.slug.as_ref(), cell.output.format));
                let up_to_date =
                    stamp::is_up_to_date(output_dir, cell.slug.as_ref(), print, &artifact);
                planned.push(PlannedCell {
                    cell,
                    fingerprint: print,
                    up_to_date,
                    base_ref: base_image_ref(&resolved),
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
        output_dir: &Path,
        options: &BuildOptions,
        cancel: CancellationToken,
        progress: &mut dyn FnMut(BuildProgress<'_>),
    ) -> Result<Vec<ExecutionResult>, CoreError> {
        let runtime = runtime_config(tool, lock);
        let mut results = Vec::new();
        for planned in &plan.cells {
            if planned.up_to_date && !options.force {
                continue;
            }
            let (toolchain_id, toolchain) = toolchain_for(&planned.cell.target, tool)?;
            let toolchain_digest = self
                .toolchain_digest(&toolchain_id, &toolchain, lock)
                .await?;
            let context = ExecutionContext {
                output_dir: output_dir.to_path_buf(),
                ic_image_ref: format!("{}@{toolchain_digest}", toolchain.container),
                base_ref: planned.base_ref.clone(),
                platform: format!("linux/{}", planned.cell.arch),
                clone_index: options.clone_index,
                dry_run: options.dry_run,
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
        output_dir: &Path,
    ) -> Result<Vec<ExecutionResult>, CoreError> {
        let runtime = runtime_config(tool, &Lockfile::default());
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
                    platform: format!("linux/{}", cell.arch),
                    clone_index: None,
                    dry_run: true,
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

    async fn toolchain_digest(
        &self,
        id: &str,
        toolchain: &ToolchainEntry,
        lock: &Lockfile,
    ) -> Result<String, CoreError> {
        match lock.toolchain_digest(id) {
            Some(digest) => Ok(digest.to_owned()),
            None => Ok(self.resolver.resolve_toolchain(toolchain).await?),
        }
    }
}

/// Resolve a target's toolchain to its `(id, entry)`, applying the workspace default if unset.
fn toolchain_for(
    target: &Target,
    tool: &ToolConfig,
) -> Result<(String, ToolchainEntry), CoreError> {
    let lookup = |id: &str| -> Result<ToolchainEntry, CoreError> {
        tool.toolchains
            .entries
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

/// Expand a target into its cells: one per (rendered matrix point × architecture × output format).
pub fn cells(target: &Arc<Target>) -> Result<Vec<Cell>, CoreError> {
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
            // No `arch` axis: a catalogue slot's `arch` supplies it when the image declares none;
            // otherwise the workspace `defaults.architectures`, else the `amd64` default.
            None => match slot_arch {
                Some(slot) if target.architectures == [Arch::Amd64] => vec![slot],
                _ if target.architectures.is_empty() => vec![Arch::Amd64],
                _ => target.architectures.clone(),
            },
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
    let BaseSource::Ref { reference } = base else {
        return Ok((base.clone(), None, None));
    };
    let slot = target
        .base_images
        .get(reference)
        .ok_or_else(|| CoreError::UnknownBaseImage {
            image: target.name().to_owned(),
            name: reference.clone(),
            known: target
                .base_images
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
        })?;
    let path = tailor_config::absolutize(&slot.path, &target.root);
    Ok((
        BaseSource::Path { path },
        Some(reference.clone()),
        slot.arch,
    ))
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

/// Build the execution runtime config from the tool config and lock (janitor digest).
pub fn runtime_config(tool: &ToolConfig, lock: &Lockfile) -> RuntimeConfig {
    let runtime = tool.runtime.as_ref();
    let host_root = runtime
        .and_then(|r| r.mounts.as_ref())
        .and_then(|m| m.host_root.clone())
        .unwrap_or_else(|| DEFAULT_HOST_ROOT.into());
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
        privileged: runtime.and_then(|r| r.privileged).unwrap_or(true),
        build_dir: runtime.and_then(|r| r.build_dir.clone()).map(Into::into),
        log_level: runtime.and_then(|r| r.log_level.map(|l| l.as_str().to_owned())),
        image_cache_dir: runtime.and_then(|r| r.image_cache_dir.clone()),
        log_dir: runtime.and_then(|r| r.log_dir.clone()),
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
            architectures: vec![Arch::Amd64, Arch::Arm64],
            default_outputs: Vec::new(),
            output_artifacts: OutputArtifactsPolicy::default(),
            root: tmp.path().to_path_buf(),
            base_images: BaseImageCatalogue::default(),
        });
        (tmp, target)
    }

    fn tool_config() -> ToolConfig {
        serde_yaml_ng::from_str(indoc! {"
            schemaVersion: 1
            toolchains:
              default: ic
              entries:
                ic:
                  container: registry.example/imagecustomizer
                  version: 1.0.0
        "})
        .unwrap()
    }

    #[tokio::test]
    async fn plans_all_cells_as_stale() {
        let orchestrator = Orchestrator::new(FakeExecutor::default(), FakeResolver);
        let (_tmp, target) = mini_target();
        let tool = tool_config();
        let lock = Lockfile::default();
        let out = TempDir::new().unwrap();

        let plan = orchestrator
            .plan(&[target], &tool, &lock, &Selector::default(), out.path())
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
        let lock = Lockfile::default();
        let out = TempDir::new().unwrap();

        let plan = orchestrator
            .plan(&[target], &tool, &lock, &Selector::default(), out.path())
            .await
            .unwrap();
        let results = orchestrator
            .build(
                &plan,
                &tool,
                &lock,
                out.path(),
                &BuildOptions::default(),
                CancellationToken::new(),
                &mut |_| {},
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
            .plan(&[target2], &tool, &lock, &Selector::default(), out.path())
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
            .dry_run(&[target], &tool_config(), &Selector::default(), out.path())
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

    /// Build a single-cell target from `body`, applying `arches` as the workspace default; an empty
    /// `arches` mimics a workspace declaring no `defaults.architectures`.
    fn target_with(body: &str, arches: Vec<Arch>) -> (TempDir, Arc<Target>) {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "image.yaml", body);
        let definition = load_image(tmp.path().join("image.yaml")).unwrap();
        let target = Arc::new(Target {
            definition,
            dir: tmp.path().to_path_buf(),
            architectures: arches,
            default_outputs: vec![OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: None,
                name: None,
            }],
            output_artifacts: OutputArtifactsPolicy::default(),
            root: tmp.path().to_path_buf(),
            base_images: BaseImageCatalogue::default(),
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
        let (_tmp, target) = target_with(NO_ARCH_IMAGE, Vec::new());
        let cells = cells(&target).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].arch, Arch::Amd64);
        assert_eq!(cells[0].slug.as_ref(), "solo_amd64_cosi");
    }

    #[test]
    fn workspace_architectures_override_default() {
        let (_tmp, target) = target_with(NO_ARCH_IMAGE, vec![Arch::Arm64]);
        let cells = cells(&target).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].arch, Arch::Arm64);
    }

    #[test]
    fn arch_axis_drives_one_cell_per_value() {
        let (_tmp, target) = target_with(
            indoc! {"
                name: solo
                matrix:
                  arch: [amd64, arm64]
                base:
                  path: ./b.img
                config:
                  os:
                    hostname: solo
            "},
            Vec::new(),
        );
        let mut arches: Vec<Arch> = cells(&target).unwrap().iter().map(|c| c.arch).collect();
        arches.sort_unstable();
        assert_eq!(arches, vec![Arch::Amd64, Arch::Arm64]);
    }

    #[test]
    fn oci_platform_mismatch_is_an_error() {
        // amd64 default cell with an arm64 base platform → reconcile error (offline, no pull).
        let (_tmp, target) = target_with(
            indoc! {"
                name: solo
                base:
                  oci:
                    uri: registry.example/base:edge
                    platform: linux/arm64
                config:
                  os:
                    hostname: solo
            "},
            Vec::new(),
        );
        assert!(matches!(
            cells(&target).unwrap_err(),
            CoreError::PlatformArchMismatch { .. }
        ));
    }

    #[test]
    fn oci_platform_matching_arch_is_accepted() {
        let (_tmp, target) = target_with(
            indoc! {"
                name: solo
                base:
                  oci:
                    uri: registry.example/base:edge
                    platform: linux/amd64
                config:
                  os:
                    hostname: solo
            "},
            Vec::new(),
        );
        assert_eq!(cells(&target).unwrap()[0].arch, Arch::Amd64);
    }

    #[test]
    fn oci_platform_matches_declared_arch_axis() {
        let (_tmp, target) = target_with(
            indoc! {"
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
            "},
            Vec::new(),
        );
        assert_eq!(cells(&target).unwrap()[0].arch, Arch::Arm64);
    }

    /// Single-cell target plus a `baseImages:` catalogue and workspace `defaults.architectures`,
    /// for `base: { ref: <name> }` resolution and §3 arch reconciliation.
    fn target_with_catalogue(
        body: &str,
        arches: Vec<Arch>,
        catalogue: BaseImageCatalogue,
    ) -> (TempDir, Arc<Target>) {
        let tmp = TempDir::new().unwrap();
        write(tmp.path(), "image.yaml", body);
        let definition = load_image(tmp.path().join("image.yaml")).unwrap();
        let target = Arc::new(Target {
            definition,
            dir: tmp.path().to_path_buf(),
            architectures: arches,
            default_outputs: vec![OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: None,
                name: None,
            }],
            output_artifacts: OutputArtifactsPolicy::default(),
            root: tmp.path().to_path_buf(),
            base_images: catalogue,
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

    fn slot(arch: Option<Arch>) -> BaseImageSlot {
        BaseImageSlot {
            path: "bases/baremetal.vhdx".into(),
            arch,
            source: None,
        }
    }

    #[test]
    fn base_image_resolves_to_root_relative_path_and_exposes_name() {
        let cat = BaseImageCatalogue::from([("baremetal".to_owned(), slot(Some(Arch::Amd64)))]);
        let (tmp, target) = target_with_catalogue(SLOT_IMAGE, vec![Arch::Amd64], cat);
        let cells = cells(&target).unwrap();
        assert_eq!(cells[0].base_image.as_deref(), Some("baremetal"));
        let BaseSource::Path { path } = &cells[0].base else {
            panic!("slot did not collapse to a path base: {:?}", cells[0].base);
        };
        assert_eq!(path, &tmp.path().join("bases/baremetal.vhdx"));
    }

    #[test]
    fn slot_arch_supplies_cell_arch_when_image_declares_none() {
        let cat = BaseImageCatalogue::from([("baremetal".to_owned(), slot(Some(Arch::Arm64)))]);
        let (_tmp, target) = target_with_catalogue(SLOT_IMAGE, vec![Arch::Amd64], cat);
        assert_eq!(cells(&target).unwrap()[0].arch, Arch::Arm64);
    }

    #[test]
    fn slot_arch_conflict_with_axis_is_an_error() {
        let cat = BaseImageCatalogue::from([("baremetal".to_owned(), slot(Some(Arch::Arm64)))]);
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
            Vec::new(),
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
        let cat = BaseImageCatalogue::from([("other".to_owned(), slot(None))]);
        let (_tmp, target) = target_with_catalogue(SLOT_IMAGE, vec![Arch::Amd64], cat);
        assert!(
            matches!(
                cells(&target).unwrap_err(),
                CoreError::UnknownBaseImage { .. }
            ),
            "expected unknown base image"
        );
    }

    #[test]
    fn sourceless_slot_arch_unset_defaults_to_amd64() {
        let cat = BaseImageCatalogue::from([("baremetal".to_owned(), slot(None))]);
        let (_tmp, target) = target_with_catalogue(SLOT_IMAGE, vec![Arch::Amd64], cat);
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
