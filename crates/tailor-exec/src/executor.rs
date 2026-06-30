use std::{fs, path::PathBuf, slice};

use tokio_util::sync::CancellationToken;
use tracing::info;

use tailor_config::OutputFormat;
use tailor_core::{
    Cell, ContainerConfig, ContainerRuntime, ExecError, ExecutionContext, ExecutionResult,
    Executor, RuntimeConfig, artifact_name,
};

use crate::{
    arg_builder, arg_builder::DEV_BIND, janitor, output_artifacts, rpm_farm, working_copy,
};

const CONTAINER_NAME_PREFIX: &str = "tailor-ic";
const RPM_FARM_PREFIX: &str = ".tailor-farm";

#[derive(Debug, Clone)]
pub struct IcExecutor<R> {
    runtime: R,
}

impl<R> IcExecutor<R> {
    pub fn new(runtime: R) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> &R {
        &self.runtime
    }
}

impl<R: ContainerRuntime> Executor for IcExecutor<R> {
    async fn execute(
        &self,
        cell: &Cell,
        context: &ExecutionContext,
        cancel: CancellationToken,
    ) -> Result<ExecutionResult, ExecError> {
        let artifact_path = context
            .output_dir
            .join(artifact_name(cell.slug.as_ref(), cell.output.format));
        if context.dry_run {
            let command =
                arg_builder::render_command(&arg_builder::build_run_command(cell, context));
            info!(cell = %cell.slug, "dry-run container invocation");
            let logs = format!("# {}\n{command}", cell.slug.as_ref());
            return Ok(ExecutionResult {
                artifact_path,
                exit_code: 0,
                logs,
            });
        }

        fs::create_dir_all(&context.output_dir).map_err(|source| ExecError::Io {
            context: format!(
                "failed to create output directory `{}`",
                context.output_dir.display()
            ),
            source,
        })?;
        if let Some(cache_dir) = &context.runtime.image_cache_dir {
            fs::create_dir_all(cache_dir).map_err(|source| ExecError::Io {
                context: format!(
                    "failed to create image cache directory `{}`",
                    cache_dir.display()
                ),
                source,
            })?;
        }
        // When on-disk persistence is opted in (§5.5), ensure the log directory exists so IC's
        // `--log-file` lands; IC writes the file itself, tailor just chowns and points at it.
        let log_file = arg_builder::log_file_path(cell, context);
        if let Some(log_dir) = &context.runtime.log_dir {
            fs::create_dir_all(log_dir).map_err(|source| ExecError::Io {
                context: format!("failed to create log directory `{}`", log_dir.display()),
                source,
            })?;
        }

        let farms = prepare_rpm_farms(cell, context)?;
        let mut run_cell = cell.clone();
        run_cell.rpm_sources = farms
            .iter()
            .map(|farm| farm.arg_path.clone())
            .collect::<Vec<_>>();

        // Relocate IC's `output.artifacts` scratch to a tailor-owned path so it does not land
        // root-owned in the source tree (`meta/docs/output-artifacts-staging.md`).
        let run_id = output_artifacts::run_id();
        let staging = output_artifacts::apply(
            &mut run_cell.ic_config,
            cell.target.output_artifacts,
            cell.slug.as_ref(),
            &run_id,
            &cell.target.dir,
            &context.output_dir,
            &context.runtime.host_root,
        );
        // Reclaim any staging dir an earlier crashed run of this cell left behind (§3.5). Safe: only
        // matches tailor's own `.tailor-stage.<slug>.*`, and same-cell concurrency in one worktree is
        // already unsupported (the working copy collides).
        if staging.is_some() {
            let stale = output_artifacts::stale_staging_dirs(&cell.target.dir, cell.slug.as_ref());
            if !stale.is_empty() {
                janitor::remove_paths(&self.runtime, &stale, &context.runtime, cancel.clone())
                    .await?;
            }
        }

        let rendered_config = working_copy::render_working_copy(&run_cell.ic_config)
            .map_err(|err| ExecError::Other(err.to_string()))?;
        let working_copy_path =
            working_copy::write_working_copy(&run_cell, &rendered_config, context.clone_index)
                .map_err(|source| ExecError::Io {
                    context: "failed to write Image Customizer working copy".to_owned(),
                    source,
                })?;

        let args = arg_builder::build_ic_args(&run_cell, context);
        self.runtime.pull_image(&context.ic_image_ref).await?;
        let result = self
            .runtime
            .create_and_run(
                ContainerConfig {
                    image_ref: context.ic_image_ref.clone(),
                    platform: context.platform.clone(),
                    name: container_name(cell, context),
                    args,
                    binds: vec![
                        arg_builder::host_root_bind(&context.runtime),
                        DEV_BIND.to_owned(),
                    ],
                    privileged: context.runtime.privileged,
                    cell_slug: cell.slug.as_ref().to_owned(),
                    log_file: log_file.clone(),
                },
                cancel.clone(),
            )
            .await?;

        let _ = fs::remove_file(&working_copy_path);

        // Reclaim IC's root-owned staging tree before propagating any IC failure, so a failed build
        // never strands root-owned scratch (§3.4): chown to the caller always; scratch also removes.
        if let Some(plan) = &staging {
            janitor::chown_paths(
                &self.runtime,
                slice::from_ref(&plan.dir),
                &context.runtime,
                cancel.clone(),
            )
            .await?;
            if plan.reclaim {
                janitor::remove_paths(
                    &self.runtime,
                    slice::from_ref(&plan.dir),
                    &context.runtime,
                    cancel.clone(),
                )
                .await?;
            }
        }

        if result.exit_code != 0 {
            return Err(ExecError::IcFailed {
                cell: cell.slug.as_ref().to_owned(),
                code: result.exit_code,
                dump: result.failure_dump.unwrap_or_default(),
            });
        }
        verify_artifact(&artifact_path, cell.output.format)?;
        let mut managed_paths = vec![artifact_path.clone()];
        managed_paths.extend(farms.iter().map(RpmFarm::repodata_path));
        // The image cache dir is written by IC inside the privileged container (root-owned); fold it
        // into the janitor sweep so the caller can read/clean it sudo-free.
        if let Some(cache_dir) = &context.runtime.image_cache_dir {
            managed_paths.push(cache_dir.clone());
        }
        // The per-cell IC log is written root-owned inside the container too; chown it so a runner can
        // read/upload it (`meta/docs/logging.md` §5.5).
        if let Some(log_file) = &log_file {
            managed_paths.push(log_file.clone());
        }
        janitor::chown_paths(&self.runtime, &managed_paths, &context.runtime, cancel).await?;
        Ok(ExecutionResult {
            artifact_path,
            exit_code: result.exit_code,
            logs: result.logs,
        })
    }

    async fn clean(
        &self,
        paths: &[PathBuf],
        runtime: &RuntimeConfig,
        cancel: CancellationToken,
    ) -> Result<(), ExecError> {
        janitor::remove_paths(&self.runtime, paths, runtime, cancel).await
    }
}

#[derive(Debug)]
struct RpmFarm {
    arg_path: PathBuf,
}

impl RpmFarm {
    fn repodata_path(&self) -> PathBuf {
        self.arg_path.join("repodata")
    }
}

fn prepare_rpm_farms(cell: &Cell, context: &ExecutionContext) -> Result<Vec<RpmFarm>, ExecError> {
    let mut farms = Vec::new();
    for (index, source) in cell.rpm_sources.iter().enumerate() {
        if source.is_dir() {
            let parent = source.parent().ok_or_else(|| {
                ExecError::Other(format!(
                    "RPM source `{}` has no parent directory",
                    source.display()
                ))
            })?;
            let dest = parent.join(farm_name(cell, context.clone_index, index));
            rpm_farm::build_rpm_farm(source, &dest).map_err(|source_err| ExecError::Io {
                context: format!("failed to build RPM farm `{}`", dest.display()),
                source: source_err,
            })?;
            farms.push(RpmFarm { arg_path: dest });
        } else {
            farms.push(RpmFarm {
                arg_path: source.clone(),
            });
        }
    }
    Ok(farms)
}

fn farm_name(cell: &Cell, clone_index: Option<u32>, index: usize) -> String {
    match clone_index {
        Some(clone) => format!(
            "{RPM_FARM_PREFIX}-{}_clone{clone}-{index}",
            cell.slug.as_ref()
        ),
        None => format!("{RPM_FARM_PREFIX}-{}-{index}", cell.slug.as_ref()),
    }
}

fn container_name(cell: &Cell, context: &ExecutionContext) -> String {
    match context.clone_index {
        Some(clone) => format!(
            "{CONTAINER_NAME_PREFIX}-{}_clone{clone}-{}",
            cell.slug.as_ref(),
            std::process::id()
        ),
        None => format!(
            "{CONTAINER_NAME_PREFIX}-{}-{}",
            cell.slug.as_ref(),
            std::process::id()
        ),
    }
}

fn verify_artifact(path: &PathBuf, format: OutputFormat) -> Result<(), ExecError> {
    let metadata = fs::metadata(path).map_err(|source| ExecError::Io {
        context: format!("Image Customizer did not produce `{}`", path.display()),
        source,
    })?;
    if format == OutputFormat::PxeDir && !metadata.is_dir() {
        return Err(ExecError::Other(format!(
            "Image Customizer output `{}` is not a directory",
            path.display()
        )));
    }
    if format != OutputFormat::PxeDir && !metadata.is_file() {
        return Err(ExecError::Other(format!(
            "Image Customizer output `{}` is not a file",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

    use serde_yaml_ng::{Mapping, Value};
    use tailor_config::{
        Arch, BaseImageCatalogue, BaseSource, ImageDefinition, OutputArtifactsPolicy, OutputSpec,
    };
    use tailor_core::{CellSlug, Target};

    use crate::container::runtime::NoopRuntime;

    fn dry_run_cell() -> Cell {
        let definition: ImageDefinition =
            serde_yaml_ng::from_str("name: sample\noperation: customize\ninjectFiles: false\n")
                .unwrap();
        Cell {
            target: Arc::new(Target {
                definition,
                dir: PathBuf::from("/images"),
                architectures: vec![Arch::Amd64],
                default_outputs: Vec::new(),
                output_artifacts: OutputArtifactsPolicy::default(),
                root: PathBuf::from("/images"),
                base_images: BaseImageCatalogue::default(),
            }),
            axes: BTreeMap::new(),
            arch: Arch::Amd64,
            output: OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: None,
                name: None,
            },
            slug: CellSlug("sample_cosi".to_owned()),
            ic_config: Value::Mapping(Mapping::default()),
            base: BaseSource::Path {
                path: PathBuf::from("/images/base.img"),
            },
            base_image: None,
            rpm_sources: Vec::new(),
        }
    }

    fn dry_run_context() -> ExecutionContext {
        ExecutionContext {
            output_dir: PathBuf::from("/out"),
            ic_image_ref: "ic@sha256:abc".to_owned(),
            base_ref: None,
            platform: "linux/amd64".to_owned(),
            clone_index: None,
            dry_run: true,
            runtime: RuntimeConfig::default(),
        }
    }

    // A dry-run renders the invocation without ever calling the runtime; `NoopRuntime` (whose
    // methods all error) proves no daemon is contacted — so `build --dry-run` is engine-free.
    #[tokio::test]
    async fn dry_run_executes_without_contacting_the_runtime() {
        let executor = IcExecutor::new(NoopRuntime);
        let result = executor
            .execute(
                &dry_run_cell(),
                &dry_run_context(),
                CancellationToken::new(),
            )
            .await
            .expect("dry-run must not require a container engine");
        assert_eq!(result.exit_code, 0);
        assert!(result.logs.contains("sample_cosi"));
    }
}
