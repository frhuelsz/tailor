//! Workspace discovery (Cargo model) — `meta/docs/design.md` §5.3.
//!
//! `tailor` walks up from the current directory to find `tailor.yaml` (the workspace root); every
//! member image (`*/image.yaml` auto-discovered at depth 1, or curated via the `images` catalogue)
//! belongs to that workspace. With no manifest, a lone `image.yaml` is a standalone image.

use std::path::{Path, PathBuf};

use crate::{
    error::ConfigError,
    loader::{load_image, load_tool_config},
    schema::{ImageDefinition, ToolConfig},
};

const MANIFEST: &str = "tailor.yaml";
const IMAGE_FILE: &str = "image.yaml";
const ALL_MEMBERS_GLOB: &str = "*";

/// A discovered image: its parsed definition and the directory holding its `image.yaml`.
#[derive(Debug, Clone)]
pub struct DiscoveredImage {
    pub definition: ImageDefinition,
    pub dir: PathBuf,
}

/// A resolved workspace: the root directory, the optional tool config, and the member images.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    /// The parsed `tailor.yaml`, or `None` in standalone mode.
    pub tool: Option<ToolConfig>,
    pub images: Vec<DiscoveredImage>,
}

impl Workspace {
    /// Find an image by name.
    pub fn image(&self, name: &str) -> Option<&DiscoveredImage> {
        self.images.iter().find(|img| img.definition.name == name)
    }
}

/// Find the nearest `tailor.yaml` at or above `start`.
pub fn find_manifest(start: impl AsRef<Path>) -> Option<PathBuf> {
    let mut dir = Some(start.as_ref());
    while let Some(current) = dir {
        let candidate = current.join(MANIFEST);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = current.parent();
    }
    None
}

/// Discover the workspace containing `start`: a `tailor.yaml` workspace (with member images), or a
/// standalone `image.yaml` if no manifest is found.
pub fn discover(start: impl AsRef<Path>) -> Result<Workspace, ConfigError> {
    let start = start.as_ref();
    if let Some(manifest) = find_manifest(start) {
        let root = manifest.parent().unwrap_or(start).to_path_buf();
        let tool = load_tool_config(&manifest)?;
        let images = discover_members(&root, &tool)?;
        return Ok(Workspace {
            root,
            tool: Some(tool),
            images,
        });
    }
    let definition = load_image(start.join(IMAGE_FILE))?;
    Ok(Workspace {
        root: start.to_path_buf(),
        tool: None,
        images: vec![DiscoveredImage {
            definition,
            dir: start.to_path_buf(),
        }],
    })
}

fn discover_members(root: &Path, tool: &ToolConfig) -> Result<Vec<DiscoveredImage>, ConfigError> {
    let catalogue = tool.images.as_ref();
    let excluded: Vec<String> = catalogue
        .map(|c| c.exclude.iter().map(|e| normalize_member(e)).collect())
        .unwrap_or_default();

    let member_dirs = match catalogue.and_then(|c| c.members.as_ref()) {
        Some(members) => {
            let mut dirs = Vec::new();
            for member in members {
                if normalize_member(member) == ALL_MEMBERS_GLOB {
                    dirs.extend(depth_one_image_dirs(root)?);
                } else {
                    dirs.push(root.join(normalize_member(member)));
                }
            }
            dirs
        }
        None => depth_one_image_dirs(root)?,
    };

    let mut images = Vec::new();
    for dir in member_dirs {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if excluded.contains(&name) {
            continue;
        }
        let image_path = dir.join(IMAGE_FILE);
        if image_path.is_file() {
            images.push(DiscoveredImage {
                definition: load_image(&image_path)?,
                dir,
            });
        }
    }
    if let Some(catalogue) = catalogue {
        for definition in &catalogue.inline {
            images.push(DiscoveredImage {
                definition: definition.clone(),
                dir: root.to_path_buf(),
            });
        }
    }
    Ok(images)
}

/// Immediate subdirectories of `root` that contain an `image.yaml`, sorted for determinism.
fn depth_one_image_dirs(root: &Path) -> Result<Vec<PathBuf>, ConfigError> {
    let mut dirs = Vec::new();
    let entries = std::fs::read_dir(root).map_err(|source| ConfigError::Read {
        path: root.to_path_buf(),
        source,
    })?;
    for entry in entries {
        let path = entry
            .map_err(|source| ConfigError::Read {
                path: root.to_path_buf(),
                source,
            })?
            .path();
        if path.is_dir() && path.join(IMAGE_FILE).is_file() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Normalize a catalogue member/exclude entry to a bare path segment (`./webserver/` ⇒ `webserver`).
fn normalize_member(entry: &str) -> String {
    entry
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    fn example(rel: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../meta/docs/examples")
            .join(rel)
    }

    #[test]
    fn auto_discovers_workspace_member_images() {
        let workspace = discover(example("workspace-two-images")).unwrap();
        assert!(workspace.tool.is_some());
        let mut names: Vec<&str> = workspace
            .images
            .iter()
            .map(|i| i.definition.name.as_str())
            .collect();
        names.sort_unstable();
        assert_eq!(names, ["database", "webserver"]);
        assert!(workspace.image("webserver").is_some());
    }

    #[test]
    fn standalone_image_without_a_manifest() {
        // Copy the image into a tempdir so the walk-up cannot escape to a parent manifest.
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::copy(
            example("minimal-single-image/image.yaml"),
            tmp.path().join("image.yaml"),
        )
        .unwrap();
        let workspace = discover(tmp.path()).unwrap();
        assert!(workspace.tool.is_none());
        assert_eq!(workspace.images.len(), 1);
    }
}
