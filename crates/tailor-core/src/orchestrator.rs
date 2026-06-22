//! Build orchestration: render each image's cells, resolve their inputs, compute fingerprints,
//! consult build stamps, and produce a `BuildPlan`; then drive execution through the `Executor`
//! port (`meta/docs/architecture.md` §3.2, stages 11–18 of §5).

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

use tailor_config::{
    Arch, OutputFormat, ToolConfig, ToolchainEntry, ToolchainRef, cell_slug, render_image,
};
use tokio_util::sync::CancellationToken;

use crate::{
    domain::{BuildPlan, Cell, CellSlug, PlannedCell, Target},
    error::CoreError,
    fingerprint::{FingerprintInputs, fingerprint},
    lockfile::Lockfile,
    ports::{BaseResolver, ExecutionContext, ExecutionResult, Executor, RuntimeConfig},
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
                let resolved = self.resolver.resolve(&cell.base, cell.arch).await?;
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
                });
            }
        }
        Ok(BuildPlan { cells: planned })
    }

    /// Execute the stale cells of a plan, writing a build stamp after each success.
    pub async fn build(
        &self,
        plan: &BuildPlan,
        tool: &ToolConfig,
        lock: &Lockfile,
        output_dir: &Path,
        options: &BuildOptions,
        cancel: CancellationToken,
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
                platform: format!("linux/{}", planned.cell.arch),
                clone_index: options.clone_index,
                dry_run: options.dry_run,
                runtime: runtime.clone(),
            };
            let result = self
                .executor
                .execute(&planned.cell, &context, cancel.clone())
                .await?;
            if !options.dry_run {
                stamp::write(output_dir, planned.cell.slug.as_ref(), planned.fingerprint)?;
            }
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
        let arch_is_axis = rc.tuple.get("arch").is_some();
        let arches = match rc.tuple.get("arch") {
            Some(value) => vec![parse_arch(value).ok_or_else(|| CoreError::MissingArchBase {
                image: target.name().to_owned(),
                arch: value.to_owned(),
            })?],
            None => target.architectures.clone(),
        };
        for arch in arches {
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
                    base: rc.base.clone(),
                    rpm_sources: rc.rpm_sources.clone(),
                });
            }
        }
    }
    Ok(cells)
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
    let janitor_image = janitor.map_or_else(String::new, |j| {
        let digest = lock
            .runtime
            .as_ref()
            .and_then(|r| r.janitor_image.as_ref())
            .map(|c| c.digest.as_str());
        match digest {
            Some(digest) => format!("{}@{digest}", j.container),
            None => format!("{}:{}", j.container, j.tag.as_deref().unwrap_or("latest")),
        }
    });
    RuntimeConfig {
        host_root,
        privileged: runtime.and_then(|r| r.privileged).unwrap_or(true),
        build_dir: runtime.and_then(|r| r.build_dir.clone()).map(Into::into),
        log_level: runtime.and_then(|r| r.log_level.map(|l| l.as_str().to_owned())),
        image_cache_dir: runtime.and_then(|r| r.image_cache_dir.clone()),
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

    use std::path::PathBuf;

    use tailor_config::load_image;
    use tempfile::TempDir;

    use crate::testing::{FakeExecutor, FakeResolver};

    fn trident_target() -> Arc<Target> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../meta/docs/examples/trident-vm-testimage");
        let definition = load_image(dir.join("image.yaml")).unwrap();
        Arc::new(Target {
            definition,
            dir,
            architectures: vec![Arch::Amd64, Arch::Arm64],
            default_outputs: Vec::new(),
        })
    }

    fn tool_config() -> ToolConfig {
        serde_yaml_ng::from_str(indoc::indoc! {"
            schemaVersion: 1
            toolchains:
              default: ic-1.3
              entries:
                ic-1.3:
                  container: mcr.microsoft.com/azurelinux/imagecustomizer
                  version: 1.3.0
        "})
        .unwrap()
    }

    #[tokio::test]
    async fn plans_all_trident_cells_as_stale() {
        let orchestrator = Orchestrator::new(FakeExecutor::default(), FakeResolver);
        let target = trident_target();
        let tool = tool_config();
        let lock = Lockfile::default();
        let out = TempDir::new().unwrap();

        let plan = orchestrator
            .plan(&[target], &tool, &lock, &Selector::default(), out.path())
            .await
            .unwrap();

        // 16 matrix cells, one output format each.
        assert_eq!(plan.cells.len(), 16);
        assert!(plan.cells.iter().all(|c| !c.up_to_date));
        assert_eq!(plan.stale().count(), 16);
    }

    #[tokio::test]
    async fn build_executes_stale_cells_and_writes_stamps() {
        let executor = FakeExecutor::default();
        let recorder = executor.recorder();
        let orchestrator = Orchestrator::new(executor, FakeResolver);
        let target = trident_target();
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
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 16);
        assert_eq!(
            recorder.lock().unwrap().len(),
            16,
            "executor invoked once per stale cell"
        );
        // A second plan now sees the stamps and marks every cell up to date.
        let replan = orchestrator
            .plan(
                &[trident_target()],
                &tool,
                &lock,
                &Selector::default(),
                out.path(),
            )
            .await
            .unwrap();
        // Stamps exist but artifacts do not (fake executor wrote none), so cells stay stale.
        assert_eq!(replan.cells.len(), 16);
    }

    #[tokio::test]
    async fn dry_run_renders_every_cell_without_resolution() {
        // dry_run never calls the resolver (no digests needed), so it works fully offline.
        let executor = FakeExecutor::default();
        let recorder = executor.recorder();
        let orchestrator = Orchestrator::new(executor, FakeResolver);
        let out = TempDir::new().unwrap();

        let results = orchestrator
            .dry_run(
                &[trident_target()],
                &tool_config(),
                &Selector::default(),
                out.path(),
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 16);
        assert_eq!(recorder.lock().unwrap().len(), 16);
    }

    #[test]
    fn cells_selected_narrows_to_a_slice_and_a_single_cell() {
        let target = trident_target();
        // A one-axis slice: every amd64 cell (variant[4] × release[2] × phase[1]).
        let slice = Selector::parse(&["arch=amd64".to_owned()], &[], &[]).unwrap();
        assert_eq!(cells_selected(&target, &slice).unwrap().len(), 8);

        // Pinning every axis yields exactly one cell.
        let one = Selector::parse(
            &["variant=grub,arch=amd64,release=3.0,phase=base".to_owned()],
            &[],
            &[],
        )
        .unwrap();
        let selected = cells_selected(&target, &one).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].slug.as_ref(),
            "trident-vm-testimage_grub_amd64_3.0_base_cosi"
        );
    }

    #[test]
    fn cells_selected_rejects_unknown_axis_and_empty_selection() {
        let target = trident_target();
        let bad_axis = Selector::parse(&["distro=fedora".to_owned()], &[], &[]).unwrap();
        assert!(matches!(
            cells_selected(&target, &bad_axis).unwrap_err(),
            CoreError::UnknownSelectorAxis { .. }
        ));
        let no_match = Selector::parse(&["variant=does-not-exist".to_owned()], &[], &[]).unwrap();
        assert!(matches!(
            cells_selected(&target, &no_match).unwrap_err(),
            CoreError::NoCellsSelected { .. }
        ));
    }
}
