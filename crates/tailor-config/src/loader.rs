use std::{fs, path::Path};

use serde::de::DeserializeOwned;

use crate::{
    error::ConfigError,
    schema::{ImageDefinition, ToolConfig},
};

/// Parse a `tailor.yaml` workspace/tool config.
pub fn load_tool_config(path: impl AsRef<Path>) -> Result<ToolConfig, ConfigError> {
    let tool_config: ToolConfig = parse_yaml(path.as_ref())?;
    tool_config.validate()?;
    Ok(tool_config)
}

/// Parse an `image.yaml` image definition (base document).
pub fn load_image(path: impl AsRef<Path>) -> Result<ImageDefinition, ConfigError> {
    parse_yaml(path.as_ref())
}

fn parse_yaml<T: DeserializeOwned>(path: &Path) -> Result<T, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_yaml_ng::from_str(&text).map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;
    use tempfile::TempDir;

    use crate::matrix;

    /// Write `body` to `<tmp>/<name>` and return the path.
    fn write(tmp: &TempDir, name: &str, body: &str) -> std::path::PathBuf {
        let path = tmp.path().join(name);
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn loads_a_tool_config_from_yaml() {
        let tmp = TempDir::new().unwrap();
        let path = write(
            &tmp,
            "tailor.yaml",
            indoc! {"
                schemaVersion: 1
                toolchains:
                  default: ic-main
                  entries:
                    - name: ic-main
                      container: registry.example/imagecustomizer
                      version: 2.0.0
                    - name: ic-old
                      container: registry.example/imagecustomizer
                      version: 1.0.0
            "},
        );
        let tc = load_tool_config(&path).unwrap();
        assert_eq!(tc.schema_version, 1);
        assert_eq!(tc.toolchains.default, "ic-main");
        assert!(tc.toolchains.get("ic-old").is_some());
    }

    #[test]
    fn duplicate_toolchain_names_are_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = write(
            &tmp,
            "tailor.yaml",
            indoc! {"
                schemaVersion: 1
                toolchains:
                  default: ic
                  entries:
                    - name: ic
                      container: registry.example/imagecustomizer
                    - name: ic
                      container: registry.example/imagecustomizer-old
            "},
        );
        let err = load_tool_config(&path).unwrap_err();
        assert!(
            matches!(&err, ConfigError::DuplicateCatalogueName { catalogue, name } if catalogue == "toolchains.entries" && name == "ic"),
            "got {err:?}"
        );
    }

    #[test]
    fn duplicate_base_image_names_are_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = write(
            &tmp,
            "tailor.yaml",
            indoc! {"
                schemaVersion: 1
                toolchains:
                  default: ic
                  entries:
                    - name: ic
                      container: registry.example/imagecustomizer
                baseImages:
                  - name: baremetal
                    path: bases/a.vhdx
                  - name: baremetal
                    path: bases/b.vhdx
            "},
        );
        let err = load_tool_config(&path).unwrap_err();
        assert!(
            matches!(&err, ConfigError::DuplicateCatalogueName { catalogue, name } if catalogue == "baseImages" && name == "baremetal"),
            "got {err:?}"
        );
    }

    #[test]
    fn loads_tools_dir_sources_from_yaml() {
        let tmp = TempDir::new().unwrap();
        let path = write(
            &tmp,
            "tailor.yaml",
            indoc! {"
                schemaVersion: 1
                toolchains:
                  default: ic
                  entries:
                    - name: ic
                      container: registry.example/imagecustomizer
                toolsDirSources:
                  - name: acl
                    container: mcr.microsoft.com/azurelinux/base/core
                    tag: '3.0'
            "},
        );
        let tc = load_tool_config(&path).unwrap();
        assert_eq!(tc.tools_dir_sources.len(), 1);
        assert_eq!(tc.tools_dir_sources[0].name, "acl");
        assert_eq!(tc.tools_dir_sources[0].effective_tag(), "3.0");
    }

    #[test]
    fn duplicate_tools_dir_source_names_are_rejected() {
        let tmp = TempDir::new().unwrap();
        let path = write(
            &tmp,
            "tailor.yaml",
            indoc! {"
                schemaVersion: 1
                toolchains:
                  default: ic
                  entries:
                    - name: ic
                      container: registry.example/imagecustomizer
                toolsDirSources:
                  - name: acl
                    container: registry.example/acl
                  - name: acl
                    container: registry.example/acl-other
            "},
        );
        let err = load_tool_config(&path).unwrap_err();
        assert!(
            matches!(&err, ConfigError::DuplicateCatalogueName { catalogue, name } if catalogue == "toolsDirSources" && name == "acl"),
            "got {err:?}"
        );
    }

    #[test]
    fn image_tools_dir_refs_and_inline_sources_parse() {
        let tmp = TempDir::new().unwrap();
        let named = write(
            &tmp,
            "named.yaml",
            indoc! {"
                name: gizmo
                base:
                  path: ./b.img
                toolsDir:
                  source: acl
                config:
                  previewFeatures:
                    - tools-dir
            "},
        );
        let image = load_image(&named).unwrap();
        let tools_dir = image.tools_dir.unwrap();
        assert_eq!(tools_dir.access, crate::schema::Access::Ro);
        assert!(
            matches!(tools_dir.source, crate::schema::ToolsDirSourceRef::Id(name) if name == "acl")
        );

        let inline = write(
            &tmp,
            "inline.yaml",
            indoc! {"
                name: gizmo
                base:
                  path: ./b.img
                toolsDir:
                  source:
                    container: quay.io/fedora/fedora
                    tag: '42'
                  access: rw
                config:
                  previewFeatures:
                    - tools-dir
            "},
        );
        let image = load_image(&inline).unwrap();
        let tools_dir = image.tools_dir.unwrap();
        assert_eq!(tools_dir.access, crate::schema::Access::Rw);
        assert!(
            matches!(tools_dir.source, crate::schema::ToolsDirSourceRef::Inline(source) if source.container == "quay.io/fedora/fedora" && source.effective_tag() == "42")
        );
    }

    #[test]
    fn loads_an_image_definition_and_expands_its_matrix() {
        let tmp = TempDir::new().unwrap();
        let path = write(
            &tmp,
            "image.yaml",
            indoc! {"
                name: gizmo
                matrix:
                  edition: [lite, pro]
                  arch: [amd64, arm64]
                base:
                  path: ./b.img
                config:
                  os:
                    hostname: gizmo
            "},
        );
        let img = load_image(&path).unwrap();
        assert_eq!(img.name, "gizmo");
        let m = img.matrix.expect("declares a matrix");
        assert_eq!(matrix::expand(&m, img.selectors.as_ref()).unwrap().len(), 4); // edition[2] × arch[2]
    }

    #[test]
    fn parse_failure_is_reported_as_a_parse_error() {
        let tmp = TempDir::new().unwrap();
        // A YAML sequence cannot deserialize into the `ImageDefinition` struct.
        let path = write(&tmp, "image.yaml", "- not\n- a\n- mapping\n");
        let err = load_image(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "got {err:?}");
    }

    #[test]
    fn a_missing_file_is_reported_as_a_read_error() {
        let err = load_image(std::path::Path::new("/no/such/dir/image.yaml")).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }), "got {err:?}");
    }
}
