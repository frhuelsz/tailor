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
const FLAG_LOG_FORMAT: &str = "--log-format";
const FLAG_LOG_COLOR: &str = "--log-color";
const FLAG_LOG_FILE: &str = "--log-file";
const LOG_FORMAT_JSON: &str = "json";
const LOG_COLOR_NEVER: &str = "never";
/// IC's `--log-level` default when the manifest/flags set none: run IC at full `debug` so the
/// in-memory capture and any on-disk log always have the full story (`meta/docs/logging.md` §5.1).
const DEFAULT_IC_LOG_LEVEL: &str = "debug";
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

    // Always run IC structured (`meta/docs/logging.md` §5.1): JSON on stderr, no ANSI, at `debug`
    // (or the configured level) so the in-memory capture and any on-disk log have the full story.
    args.extend(flag_value(FLAG_LOG_FORMAT, LOG_FORMAT_JSON.to_owned()));
    args.extend(flag_value(FLAG_LOG_COLOR, LOG_COLOR_NEVER.to_owned()));
    args.extend(flag_value(
        FLAG_LOG_LEVEL,
        context
            .runtime
            .log_level
            .clone()
            .unwrap_or_else(|| DEFAULT_IC_LOG_LEVEL.to_owned()),
    ));
    // Opt-in on-disk persistence (§5.5): write the per-cell debug log into the container path that
    // maps to `<log-dir>/<slug>.log` on the host.
    if let Some(log_file) = log_file_path(cell, context) {
        args.extend(flag_value(
            FLAG_LOG_FILE,
            path_translate::to_container_path(&log_file, &context.runtime.host_root),
        ));
    }

    args.extend(flag_value(FLAG_BUILD_DIR, BUILD_DIR.to_owned()));
    args.extend(base_args(
        &cell.base,
        context.base_ref.as_deref(),
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
                    &tailor_config::absolutize(source, &cell.target.dir),
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

/// The host path of the per-cell IC log when on-disk persistence is enabled, else `None`
/// (`meta/docs/logging.md` §5.5). A `--clones` run suffixes the slug so clones never collide.
pub(crate) fn log_file_path(cell: &Cell, context: &ExecutionContext) -> Option<PathBuf> {
    context.runtime.log_dir.as_ref().map(|dir| {
        let name = match context.clone_index {
            Some(clone) => format!("{}_clone{clone}.log", cell.slug.as_ref()),
            None => format!("{}.log", cell.slug.as_ref()),
        };
        dir.join(name)
    })
}

fn subcommand(operation: Operation) -> &'static str {
    match operation {
        Operation::Customize => SUBCOMMAND_CUSTOMIZE,
        Operation::Convert => SUBCOMMAND_CONVERT,
    }
}

fn base_args(
    base: &BaseSource,
    base_ref: Option<&str>,
    image_dir: &std::path::Path,
    runtime: &RuntimeConfig,
    operation: Operation,
) -> Vec<String> {
    match (base, operation) {
        (BaseSource::Path { path }, _) => flag_value(
            FLAG_IMAGE_FILE,
            path_translate::to_container_path(
                &tailor_config::absolutize(path, image_dir),
                &runtime.host_root,
            ),
        ),
        // A registry base: pass the digest-pinned `oci:<repo>@<digest>` resolved during planning
        // (reproducible). `--dry-run` resolves no digest, so fall back to the un-pinned reference for
        // the preview only.
        (BaseSource::Oci { oci }, Operation::Customize) => flag_value(
            FLAG_IMAGE,
            base_ref.map_or_else(|| format!("{OCI_PREFIX}{}", oci.uri), ToOwned::to_owned),
        ),
        (BaseSource::AzureLinux { azure_linux }, Operation::Customize) => flag_value(
            FLAG_IMAGE,
            base_ref.map_or_else(
                || {
                    format!(
                        "{OCI_PREFIX}mcr.microsoft.com/azurelinux/{}/image/{}",
                        azure_linux.version, azure_linux.variant
                    )
                },
                ToOwned::to_owned,
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
        // A catalogue reference is collapsed to a `path` base during cell expansion, so it never
        // reaches the argument builder; treat its absolute path like a `path` base if it does.
        (BaseSource::Ref { reference }, _) => flag_value(FLAG_IMAGE_FILE, reference.clone()),
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
        Arch, BaseImageCatalogue, BaseSource, ImageDefinition, OciBase, Operation,
        OutputArtifactsPolicy, OutputFormat, OutputSpec,
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
                "--log-format",
                "json",
                "--log-color",
                "never",
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

    #[test]
    fn customize_uses_the_pinned_base_ref_when_present() {
        // A real build threads the digest-pinned `base_ref`; it must win over the manifest uri.
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::AzureLinux {
                azure_linux: tailor_config::AzureLinuxBase {
                    version: "3.0".to_owned(),
                    variant: "minimal-os".to_owned(),
                },
            },
        );
        let mut context = sample_context();
        context.base_ref =
            Some("oci:mcr.microsoft.com/azurelinux/3.0/image/minimal-os@sha256:dead".to_owned());

        let args = build_ic_args(&cell, &context);

        assert!(args.windows(2).any(|pair| pair
            == [
                FLAG_IMAGE,
                "oci:mcr.microsoft.com/azurelinux/3.0/image/minimal-os@sha256:dead"
            ]));
    }

    #[test]
    fn always_runs_ic_structured_at_debug_by_default() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
            },
        );
        let mut context = sample_context();
        // No explicit log level: IC must still run structured at the `debug` default (§5.1).
        context.runtime.log_level = None;

        let args = build_ic_args(&cell, &context);

        assert!(args.windows(2).any(|p| p == [FLAG_LOG_FORMAT, "json"]));
        assert!(args.windows(2).any(|p| p == [FLAG_LOG_COLOR, "never"]));
        assert!(args.windows(2).any(|p| p == [FLAG_LOG_LEVEL, "debug"]));
        // Persistence is off by default: no `--log-file`.
        assert!(!args.iter().any(|arg| arg == FLAG_LOG_FILE));
    }

    #[test]
    fn persists_per_cell_log_when_log_dir_is_set() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
            },
        );
        let mut context = sample_context();
        context.runtime.log_dir = Some(PathBuf::from("/logs"));

        let args = build_ic_args(&cell, &context);

        assert!(
            args.windows(2)
                .any(|p| p == [FLAG_LOG_FILE, "/host/logs/sample_cosi.log"])
        );
    }

    fn sample_context() -> ExecutionContext {
        ExecutionContext {
            output_dir: PathBuf::from("/out"),
            ic_image_ref: "ic@sha256:abc".to_owned(),
            base_ref: None,
            platform: "linux/amd64".to_owned(),
            clone_index: None,
            dry_run: false,
            runtime: RuntimeConfig {
                host_root: PathBuf::from("/host"),
                privileged: true,
                build_dir: None,
                log_level: Some("debug".to_owned()),
                image_cache_dir: Some(PathBuf::from("/cache")),
                log_dir: None,
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
                output_artifacts: OutputArtifactsPolicy::default(),
                root: PathBuf::from("/images"),
                base_images: BaseImageCatalogue::default(),
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
            base_image: None,
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
