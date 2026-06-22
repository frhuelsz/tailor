use std::fmt::Write as _;

use bollard::{
    Docker,
    container::{
        AttachContainerOptions, Config as DockerConfig, CreateContainerOptions,
        RemoveContainerOptions, WaitContainerOptions,
    },
    errors::Error as BollardError,
    image::CreateImageOptions,
    models::HostConfig,
};
use futures_util::{StreamExt, TryStreamExt};
use tokio::{select, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::info;

use tailor_core::{ContainerConfig, ContainerResult, ContainerRuntime, DaemonInfo, ExecError};

const ATTACH_LOGS: bool = true;
const WAIT_CONDITION_NOT_RUNNING: &str = "not-running";

#[derive(Debug, Clone)]
pub struct BollardRuntime {
    docker: Docker,
}

impl BollardRuntime {
    pub fn connect() -> Result<Self, ExecError> {
        Docker::connect_with_local_defaults()
            .map(|docker| Self { docker })
            .map_err(|err| map_runtime_error(&err))
    }
}

impl ContainerRuntime for BollardRuntime {
    async fn pull_image(&self, reference: &str) -> Result<(), ExecError> {
        self.docker
            .create_image(
                Some(CreateImageOptions {
                    from_image: reference,
                    ..Default::default()
                }),
                None,
                None,
            )
            .try_collect::<Vec<_>>()
            .await
            .map(|_| ())
            .map_err(|err| map_runtime_error(&err))
    }

    async fn create_and_run(
        &self,
        config: ContainerConfig,
        cancel: CancellationToken,
    ) -> Result<ContainerResult, ExecError> {
        let name = config.name.clone();
        let docker_config = DockerConfig {
            image: Some(config.image_ref),
            cmd: Some(config.args),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            host_config: Some(HostConfig {
                binds: Some(config.binds),
                privileged: Some(config.privileged),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: name.clone(),
                    platform: Some(config.platform),
                }),
                docker_config,
            )
            .await
            .map_err(|err| map_runtime_error(&err))?;

        let attach = self
            .docker
            .attach_container(
                &name,
                Some(AttachContainerOptions::<String> {
                    stdout: Some(ATTACH_LOGS),
                    stderr: Some(ATTACH_LOGS),
                    stream: Some(ATTACH_LOGS),
                    logs: Some(ATTACH_LOGS),
                    ..Default::default()
                }),
            )
            .await
            .map_err(|err| map_runtime_error(&err))?;
        let log_task = stream_logs(attach.output);

        self.docker
            .start_container::<String>(&name, None)
            .await
            .map_err(|err| map_runtime_error(&err))?;

        let wait = wait_for_container(self.docker.clone(), name.clone());
        let exit_code = select! {
            result = wait => result?,
            () = cancel.cancelled() => {
                remove_container(&self.docker, &name).await?;
                return Err(ExecError::Cancelled);
            }
        };
        remove_container(&self.docker, &name).await?;
        let logs = log_task
            .await
            .map_err(|err| ExecError::Runtime(err.to_string()))?;
        Ok(ContainerResult { exit_code, logs })
    }

    async fn daemon_info(&self) -> Result<DaemonInfo, ExecError> {
        let info = self
            .docker
            .info()
            .await
            .map_err(|err| map_runtime_error(&err))?;
        let options = info.security_options.unwrap_or_default();
        Ok(DaemonInfo {
            rootless: options.iter().any(|item| item.contains("rootless")),
            userns_remap: options.iter().any(|item| item.contains("userns")),
        })
    }
}

async fn wait_for_container(docker: Docker, name: String) -> Result<i64, ExecError> {
    let mut stream = docker.wait_container(
        &name,
        Some(WaitContainerOptions {
            condition: WAIT_CONDITION_NOT_RUNNING,
        }),
    );
    match stream.next().await {
        Some(Ok(response)) => Ok(response.status_code),
        Some(Err(BollardError::DockerContainerWaitError { code, .. })) => Ok(code),
        Some(Err(err)) => Err(map_runtime_error(&err)),
        None => Err(ExecError::Runtime(
            "container wait stream ended without a status".to_owned(),
        )),
    }
}

fn stream_logs(
    mut output: std::pin::Pin<
        Box<
            dyn futures_util::Stream<Item = Result<bollard::container::LogOutput, BollardError>>
                + Send,
        >,
    >,
) -> JoinHandle<String> {
    tokio::spawn(async move {
        let mut logs = String::new();
        while let Some(item) = output.next().await {
            match item {
                Ok(line) => {
                    let line = line.to_string();
                    info!(message = %line, "container log");
                    logs.push_str(&line);
                }
                Err(err) => {
                    let _ = writeln!(logs, "container log stream error: {err}");
                }
            }
        }
        logs
    })
}

async fn remove_container(docker: &Docker, name: &str) -> Result<(), ExecError> {
    docker
        .remove_container(
            name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|err| map_runtime_error(&err))
}

fn map_runtime_error(err: &BollardError) -> ExecError {
    ExecError::Runtime(err.to_string())
}
