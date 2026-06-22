use std::{fs, path::Path};

use serde::de::DeserializeOwned;

use crate::{
    error::ConfigError,
    schema::{ImageDefinition, ToolConfig},
};

/// Parse a `tailor.yaml` workspace/tool config.
pub fn load_tool_config(path: impl AsRef<Path>) -> Result<ToolConfig, ConfigError> {
    parse_yaml(path.as_ref())
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
                    ic-main:
                      container: registry.example/imagecustomizer
                      version: 2.0.0
                    ic-old:
                      container: registry.example/imagecustomizer
                      version: 1.0.0
            "},
        );
        let tc = load_tool_config(&path).unwrap();
        assert_eq!(tc.schema_version, 1);
        assert_eq!(tc.toolchains.default, "ic-main");
        assert!(tc.toolchains.entries.contains_key("ic-old"));
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
        assert_eq!(matrix::expand(&m).unwrap().len(), 4); // edition[2] × arch[2]
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
