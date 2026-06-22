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

    use std::path::PathBuf;

    use crate::matrix;

    fn example(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../meta/docs/examples")
            .join(rel)
    }

    #[test]
    fn loads_workspace_tool_config() {
        let tc = load_tool_config(example("workspace-two-images/tailor.yaml")).unwrap();
        assert_eq!(tc.schema_version, 1);
        assert_eq!(tc.toolchains.default, "ic-1.3");
        assert!(tc.toolchains.entries.contains_key("ic-1.1"));
    }

    #[test]
    fn loads_every_example_base_image() {
        for rel in [
            "minimal-single-image/image.yaml",
            "standalone-image/image.yaml",
            "workspace-two-images/webserver/image.yaml",
            "workspace-two-images/database/image.yaml",
            "trident-vm-testimage/image.yaml",
        ] {
            load_image(example(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        }
    }

    #[test]
    fn trident_matrix_expands_to_16_cells() {
        let img = load_image(example("trident-vm-testimage/image.yaml")).unwrap();
        let m = img.matrix.expect("trident-vm-testimage declares a matrix");
        // variant[4] × arch[2] × release[2] × phase[1]
        assert_eq!(matrix::expand(&m).unwrap().len(), 16);
    }
}
