use std::path::{Path, PathBuf};

use nix::unistd::{Gid, Uid};
use tokio_util::sync::CancellationToken;

use tailor_core::{ContainerConfig, ContainerRuntime, ExecError, LogSource, RuntimeConfig};

const CHOWN_BINARY: &str = "/bin/chown";
const RM_BINARY: &str = "/bin/rm";
const NAME_PREFIX: &str = "tailor-janitor";

/// The container platform the janitor runs as: the **host's native architecture**.
///
/// The janitor only runs `chown`/`rm` against bind-mounted host files, so it must run as a
/// container the host can execute natively — its architecture is irrelevant to the file operation
/// but decides whether the container can run at all. This must NOT be hardcoded: a fixed
/// `linux/amd64` fails with a 404 ("image ... does not provide the specified platform
/// (linux/amd64)") on an arm64 host, aborting an otherwise-successful build during cleanup.
///
/// Host arch (not the cell arch) is correct because the janitor operates on the host: it also fixes
/// `tailor clean` (no single cell arch) and a cross-arch build (an arm64 cell on an amd64 host still
/// cleans up natively as amd64, not under emulation). The janitor image is multi-arch and pinned by
/// its manifest-list tag/digest, so `--platform` selects the matching sub-manifest.
fn host_platform() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "linux/arm64",
        // tailor targets amd64/arm64; treat x86_64 and any other host as amd64.
        _ => "linux/amd64",
    }
}

pub(crate) async fn chown_paths<R: ContainerRuntime>(
    runtime: &R,
    paths: &[PathBuf],
    config: &RuntimeConfig,
    cancel: CancellationToken,
) -> Result<(), ExecError> {
    let existing = existing_paths(paths);
    if existing.is_empty() {
        return Ok(());
    }
    let uid = Uid::current().as_raw();
    let gid = Gid::current().as_raw();
    let mut args = vec![
        CHOWN_BINARY.to_owned(),
        "-h".to_owned(),
        "-R".to_owned(),
        format!("{uid}:{gid}"),
        "--".to_owned(),
    ];
    args.extend(existing.iter().map(|path| path_arg(path)));
    run_janitor(runtime, config, args, &existing, cancel).await
}

pub(crate) async fn remove_paths<R: ContainerRuntime>(
    runtime: &R,
    paths: &[PathBuf],
    config: &RuntimeConfig,
    cancel: CancellationToken,
) -> Result<(), ExecError> {
    let existing = existing_paths(paths);
    if existing.is_empty() {
        return Ok(());
    }
    let mut args = vec![RM_BINARY.to_owned(), "-rf".to_owned(), "--".to_owned()];
    args.extend(existing.iter().map(|path| path_arg(path)));
    run_janitor(runtime, config, args, &existing, cancel).await
}

async fn run_janitor<R: ContainerRuntime>(
    runtime: &R,
    runtime_config: &RuntimeConfig,
    args: Vec<String>,
    paths: &[PathBuf],
    cancel: CancellationToken,
) -> Result<(), ExecError> {
    if runtime_config.janitor_image.is_empty() {
        return Err(ExecError::Runtime(
            "no janitor image configured: set `runtime.janitorImage` in tailor.yaml to normalize \
             ownership of Image Customizer's root-owned outputs"
                .to_owned(),
        ));
    }
    runtime.pull_image(&runtime_config.janitor_image).await?;
    let result = runtime
        .create_and_run(
            ContainerConfig {
                image_ref: runtime_config.janitor_image.clone(),
                platform: host_platform().to_owned(),
                name: format!("{NAME_PREFIX}-{}", std::process::id()),
                args,
                binds: paths.iter().map(|path| identity_bind(path)).collect(),
                privileged: false,
                cell_slug: String::new(),
                log_source: LogSource::Janitor,
                log_file: None,
            },
            cancel,
        )
        .await?;
    if result.exit_code == 0 {
        Ok(())
    } else {
        Err(ExecError::Runtime(format!(
            "janitor exited with code {}: {}",
            result.exit_code, result.logs
        )))
    }
}

fn existing_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|path| path.exists()).cloned().collect()
}

fn identity_bind(path: &Path) -> String {
    let path = path.to_string_lossy();
    format!("{path}:{path}")
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_platform_matches_the_running_arch() {
        let platform = host_platform();
        assert!(
            platform == "linux/amd64" || platform == "linux/arm64",
            "got {platform}"
        );
        #[cfg(target_arch = "aarch64")]
        assert_eq!(platform, "linux/arm64");
        #[cfg(target_arch = "x86_64")]
        assert_eq!(platform, "linux/amd64");
    }
}
