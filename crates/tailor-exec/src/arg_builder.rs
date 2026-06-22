use std::path::PathBuf;

use tailor_config::{BaseSource, Operation};
use tailor_core::{Cell, ExecutionContext, RuntimeConfig, artifact_name};

use crate::{path_translate, working_copy};

const BUILD_DIR: &str = "/tmp";
const DOCKER: &str = "docker";
const DOCKER_RUN: &str = "run";
const FLAG_RM: &str = "--rm";
const FLAG_PRIVILEGED: &str = "--privileged";
const FLAG_PLATFORM: &str = "--platform";
const FLAG_VOLUME: &str = "-v";
pub(crate) const DEV_BIND: &str = "/dev:/dev";
const HOST_ROOT_SOURCE: &str = "/";
const FLAG_BUILD_DIR: &str = "--build-dir";
const FLAG_CONFIG_FILE: &str = "--config-file";
const FLAG_COSI_COMPRESSION_LEVEL: &str = "--cosi-compression-level";
const FLAG_IMAGE: &str = "--image";
const FLAG_IMAGE_CACHE_DIR: &str = "--image-cache-dir";
const FLAG_IMAGE_FILE: &str = "--image-file";
const FLAG_LOG_LEVEL: &str = "--log-level";
const FLAG_OUTPUT_IMAGE_FILE: &str = "--output-image-file";
const FLAG_OUTPUT_IMAGE_FORMAT: &str = "--output-image-format";
const FLAG_RPM_SOURCE: &str = "--rpm-source";
const OCI_PREFIX: &str = "oci:";
const SUBCOMMAND_CONVERT: &str = "convert";
const SUBCOMMAND_CUSTOMIZE: &str = "customize";

pub fn build_ic_args(cell: &Cell, context: &ExecutionContext) -> Vec<String> {
    let operation = cell
        .target
        .definition
        .operation
        .unwrap_or(Operation::Customize);
    let mut args = vec![subcommand(operation).to_owned()];

    if operation == Operation::Customize {
        args.extend(flag_value(
            FLAG_CONFIG_FILE,
            path_translate::to_container_path(
                &working_copy::working_copy_path(cell, context.clone_index),
                &context.runtime.host_root,
            ),
        ));
    }

    if let Some(log_level) = &context.runtime.log_level {
        args.extend(flag_value(FLAG_LOG_LEVEL, log_level.clone()));
    }

    args.extend(flag_value(FLAG_BUILD_DIR, BUILD_DIR.to_owned()));
    args.extend(base_args(
        &cell.base,
        &cell.target.dir,
        &context.runtime,
        operation,
    ));
    args.extend(flag_value(
        FLAG_OUTPUT_IMAGE_FORMAT,
        cell.output.format.as_str().to_owned(),
    ));
    args.extend(flag_value(
        FLAG_OUTPUT_IMAGE_FILE,
        path_translate::to_container_path(
            &artifact_path(cell, context),
            &context.runtime.host_root,
        ),
    ));

    if let Some(level) = cell.output.cosi_compression_level {
        args.extend(flag_value(FLAG_COSI_COMPRESSION_LEVEL, level.to_string()));
    }

    if operation == Operation::Customize {
        for source in &cell.rpm_sources {
            args.extend(flag_value(
                FLAG_RPM_SOURCE,
                path_translate::to_container_path(
                    &path_translate::absolutize(source, &cell.target.dir),
                    &context.runtime.host_root,
                ),
            ));
        }
    }

    if let Some(cache_dir) = &context.runtime.image_cache_dir {
        args.extend(flag_value(
            FLAG_IMAGE_CACHE_DIR,
            path_translate::to_container_path(cache_dir, &context.runtime.host_root),
        ));
    }

    args
}

pub(crate) fn artifact_path(cell: &Cell, context: &ExecutionContext) -> PathBuf {
    context
        .output_dir
        .join(artifact_name(cell.slug.as_ref(), cell.output.format))
}

fn subcommand(operation: Operation) -> &'static str {
    match operation {
        Operation::Customize => SUBCOMMAND_CUSTOMIZE,
        Operation::Convert => SUBCOMMAND_CONVERT,
    }
}

fn base_args(
    base: &BaseSource,
    image_dir: &std::path::Path,
    runtime: &RuntimeConfig,
    operation: Operation,
) -> Vec<String> {
    match (base, operation) {
        (BaseSource::Path { path }, _) => flag_value(
            FLAG_IMAGE_FILE,
            path_translate::to_container_path(
                &path_translate::absolutize(path, image_dir),
                &runtime.host_root,
            ),
        ),
        (BaseSource::Oci { oci }, Operation::Customize) => {
            flag_value(FLAG_IMAGE, format!("{OCI_PREFIX}{}", oci.uri))
        }
        (BaseSource::AzureLinux { azure_linux }, Operation::Customize) => flag_value(
            FLAG_IMAGE,
            format!(
                "{OCI_PREFIX}mcr.microsoft.com/azurelinux/{}/image/{}",
                azure_linux.version, azure_linux.variant
            ),
        ),
        (BaseSource::Oci { oci }, Operation::Convert) => {
            flag_value(FLAG_IMAGE_FILE, oci.uri.clone())
        }
        (BaseSource::AzureLinux { azure_linux }, Operation::Convert) => flag_value(
            FLAG_IMAGE_FILE,
            format!(
                "mcr.microsoft.com/azurelinux/{}/image/{}",
                azure_linux.version, azure_linux.variant
            ),
        ),
    }
}

fn flag_value(flag: &str, value: String) -> Vec<String> {
    vec![flag.to_owned(), value]
}

/// The host-root bind (`/:<host_root>`) that maps the host filesystem into the container (§7.3).
pub(crate) fn host_root_bind(runtime: &RuntimeConfig) -> String {
    format!("{HOST_ROOT_SOURCE}:{}", runtime.host_root.to_string_lossy())
}

/// The full container invocation tailor runs for a cell: `docker run … <image> <ic-args…>`.
///
/// This is what `--dry-run` prints. It mirrors what the bollard runtime executes (the same binds,
/// privilege, platform, image, and Image Customizer argument vector) so the preview is faithful;
/// the ephemeral `--name` is omitted for readability.
pub(crate) fn build_run_command(cell: &Cell, context: &ExecutionContext) -> Vec<String> {
    let mut argv = vec![DOCKER.to_owned(), DOCKER_RUN.to_owned(), FLAG_RM.to_owned()];
    if context.runtime.privileged {
        argv.push(FLAG_PRIVILEGED.to_owned());
    }
    argv.extend([FLAG_PLATFORM.to_owned(), context.platform.clone()]);
    argv.extend([FLAG_VOLUME.to_owned(), host_root_bind(&context.runtime)]);
    argv.extend([FLAG_VOLUME.to_owned(), DEV_BIND.to_owned()]);
    argv.push(context.ic_image_ref.clone());
    argv.extend(build_ic_args(cell, context));
    argv
}

/// Render an argv as a copy-pasteable multiline shell command (bash backslash continuations), with
/// `docker run` on the first line and each flag/value pair or bare token indented on its own line.
pub(crate) fn render_command(argv: &[String]) -> String {
    if argv.len() < 2 {
        return argv.join(" ");
    }
    let mut out = format!("{} {}", argv[0], argv[1]);
    for unit in group_tokens(&argv[2..]) {
        out.push_str(" \\\n  ");
        out.push_str(&unit);
    }
    out
}

/// Group a token slice so a flag and its value share a line, while bare tokens stand alone.
fn group_tokens(tokens: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let next_is_value = tokens
            .get(index + 1)
            .is_some_and(|next| !next.starts_with('-'));
        if token.starts_with('-') && next_is_value {
            lines.push(format!("{token} {}", tokens[index + 1]));
            index += 2;
        } else {
            lines.push(token.clone());
            index += 1;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

    use serde_yaml_ng::{Mapping, Value};
    use tailor_config::{
        Arch, BaseSource, ImageDefinition, OciBase, Operation, OutputFormat, OutputSpec,
    };
    use tailor_core::{Cell, CellSlug, ExecutionContext, RuntimeConfig, Target};

    #[test]
    fn builds_customize_args_with_translated_paths() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
            },
        );
        let context = sample_context();

        let args = build_ic_args(&cell, &context);

        assert_eq!(
            args,
            [
                "customize",
                "--config-file",
                "/host/images/.tailor-render.sample_cosi.ic.yaml",
                "--log-level",
                "debug",
                "--build-dir",
                "/tmp",
                "--image-file",
                "/host/base.raw",
                "--output-image-format",
                "cosi",
                "--output-image-file",
                "/host/out/sample_cosi.cosi",
                "--cosi-compression-level",
                "7",
                "--rpm-source",
                "/host/rpms/one",
                "--image-cache-dir",
                "/host/cache",
            ]
        );
    }

    #[test]
    fn builds_convert_args_without_config_or_rpm_sources() {
        let cell = sample_cell(
            Operation::Convert,
            BaseSource::Path {
                path: "/base.vhdx".into(),
            },
        );
        let context = sample_context();

        let args = build_ic_args(&cell, &context);

        assert!(!args.iter().any(|arg| arg == FLAG_CONFIG_FILE));
        assert!(!args.iter().any(|arg| arg == FLAG_RPM_SOURCE));
        assert_eq!(args[0], SUBCOMMAND_CONVERT);
        assert!(
            args.windows(2)
                .any(|pair| pair == [FLAG_IMAGE_FILE, "/host/base.vhdx"])
        );
    }

    #[test]
    fn run_command_carries_the_docker_prelude() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
            },
        );
        let context = sample_context();

        let argv = build_run_command(&cell, &context);

        assert_eq!(&argv[..3], ["docker", "run", "--rm"]);
        assert!(argv.iter().any(|t| t == "--privileged"));
        assert!(argv.windows(2).any(|p| p == ["--platform", "linux/amd64"]));
        assert!(argv.windows(2).any(|p| p == ["-v", "/:/host"]));
        assert!(argv.windows(2).any(|p| p == ["-v", "/dev:/dev"]));
        // The IC image precedes its subcommand and arguments.
        let image = argv.iter().position(|t| t == "ic@sha256:abc").unwrap();
        let customize = argv.iter().position(|t| t == "customize").unwrap();
        assert!(image < customize);
    }

    #[test]
    fn render_command_groups_flags_and_values_per_line() {
        let argv = vec![
            "docker".to_owned(),
            "run".to_owned(),
            "--rm".to_owned(),
            "--platform".to_owned(),
            "linux/amd64".to_owned(),
            "image".to_owned(),
            "customize".to_owned(),
            "--config-file".to_owned(),
            "/host/x.yaml".to_owned(),
        ];
        let rendered = render_command(&argv);
        let expected = "docker run \\\n  --rm \\\n  --platform linux/amd64 \\\n  image \\\n  \
             customize \\\n  --config-file /host/x.yaml";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn customize_registry_base_uses_image_flag() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Oci {
                oci: OciBase {
                    uri: "registry.example/base@sha256:abc".to_owned(),
                    platform: Some("linux/amd64".to_owned()),
                },
            },
        );
        let context = sample_context();

        let args = build_ic_args(&cell, &context);

        assert!(
            args.windows(2)
                .any(|pair| pair == [FLAG_IMAGE, "oci:registry.example/base@sha256:abc"])
        );
    }

    fn sample_context() -> ExecutionContext {
        ExecutionContext {
            output_dir: PathBuf::from("/out"),
            ic_image_ref: "ic@sha256:abc".to_owned(),
            platform: "linux/amd64".to_owned(),
            clone_index: None,
            dry_run: false,
            runtime: RuntimeConfig {
                host_root: PathBuf::from("/host"),
                privileged: true,
                build_dir: None,
                log_level: Some("debug".to_owned()),
                image_cache_dir: Some(PathBuf::from("/cache")),
                janitor_image: "janitor@sha256:def".to_owned(),
            },
        }
    }

    fn sample_cell(operation: Operation, base: BaseSource) -> Cell {
        Cell {
            target: Arc::new(Target {
                definition: sample_definition(operation),
                dir: PathBuf::from("/images"),
                architectures: vec![Arch::Amd64],
                default_outputs: Vec::new(),
            }),
            axes: BTreeMap::new(),
            arch: Arch::Amd64,
            output: OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: Some(7),
                name: None,
            },
            slug: CellSlug("sample_cosi".to_owned()),
            ic_config: Value::Mapping(Mapping::default()),
            base,
            rpm_sources: vec![PathBuf::from("/rpms/one")],
        }
    }

    fn sample_definition(operation: Operation) -> ImageDefinition {
        let operation = match operation {
            Operation::Customize => "customize",
            Operation::Convert => "convert",
        };
        serde_yaml_ng::from_str(&format!(
            r"
name: sample
operation: {operation}
injectFiles: false
"
        ))
        .unwrap()
    }
}
