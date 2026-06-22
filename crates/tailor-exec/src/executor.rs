use std::{fs, path::PathBuf};

use tokio_util::sync::CancellationToken;
use tracing::info;

use tailor_config::OutputFormat;
use tailor_core::{
    Cell, ContainerConfig, ContainerRuntime, ExecError, ExecutionContext, ExecutionResult,
    Executor, RuntimeConfig, artifact_name,
};

use crate::{arg_builder, arg_builder::DEV_BIND, janitor, rpm_farm, working_copy};

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

        let farms = prepare_rpm_farms(cell, context)?;
        let mut run_cell = cell.clone();
        run_cell.rpm_sources = farms
            .iter()
            .map(|farm| farm.arg_path.clone())
            .collect::<Vec<_>>();
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
                },
                cancel.clone(),
            )
            .await?;

        let _ = fs::remove_file(&working_copy_path);
        if result.exit_code != 0 {
            return Err(ExecError::IcFailed {
                code: result.exit_code,
                logs: result.logs,
            });
        }
        verify_artifact(&artifact_path, cell.output.format)?;
        let mut managed_paths = vec![artifact_path.clone()];
        managed_paths.extend(farms.iter().map(RpmFarm::repodata_path));
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
