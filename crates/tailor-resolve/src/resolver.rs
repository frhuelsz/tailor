use std::path::Path;

use tailor_config::{Arch, BaseSource, ToolchainEntry};
use tailor_core::{BaseResolver, ResolveError, ResolvedBase};

use crate::{azure_linux, local, oci, toolchain};

#[derive(Debug, Default, Clone, Copy)]
pub struct OciResolver;

impl OciResolver {
    pub fn new() -> Self {
        Self
    }
}

impl BaseResolver for OciResolver {
    async fn resolve(
        &self,
        source: &BaseSource,
        arch: Arch,
        image_dir: &Path,
    ) -> Result<ResolvedBase, ResolveError> {
        match source {
            // A relative `path` is authored relative to the image directory; resolve it against
            // `image_dir` (never the process CWD) so this hash/existence check sees the same file IC
            // will (`--image-file` is built the same way in tailor-exec).
            BaseSource::Path { path, .. } => {
                local::resolve(tailor_config::absolutize(path, image_dir)).await
            }
            BaseSource::Oci { oci } => oci::resolve(oci, arch).await,
            BaseSource::AzureLinux { azure_linux } => azure_linux::resolve(azure_linux, arch).await,
            // Catalogue references are collapsed to a `path` base before resolution (orchestrator).
            BaseSource::Ref { reference } => Err(ResolveError::Other(format!(
                "unresolved base reference `{reference}`: a catalogue reference must be expanded before resolution"
            ))),
        }
    }

    async fn resolve_toolchain(&self, toolchain: &ToolchainEntry) -> Result<String, ResolveError> {
        toolchain::resolve(toolchain).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    #[tokio::test]
    async fn dispatches_local_sources() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("base.raw");
        let content = b"resolver dispatch";
        fs::write(&path, content).unwrap();
        let source = BaseSource::Path { path, arch: None };

        let resolved = OciResolver::new()
            .resolve(&source, Arch::Amd64, dir.path())
            .await
            .unwrap();

        let expected: [u8; 32] = Sha256::digest(content).into();
        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                sha256: expected,
                size: content.len() as u64,
            }
        );
    }

    /// Regression: a relative `base.path` must be resolved against the image directory, not the
    /// process CWD. The base file lives one level *up* from the image dir (`<root>/artifacts/...`),
    /// reached via `../artifacts/...` — which only resolves correctly when joined onto `image_dir`.
    #[tokio::test]
    async fn resolves_relative_path_against_image_dir_not_cwd() {
        let root = tempdir().unwrap();
        let image_dir = root.path().join("image");
        let artifacts = root.path().join("artifacts");
        fs::create_dir_all(&image_dir).unwrap();
        fs::create_dir_all(&artifacts).unwrap();
        let content = b"baremetal base";
        fs::write(artifacts.join("base.raw"), content).unwrap();

        let source = BaseSource::Path {
            path: "../artifacts/base.raw".into(),
            arch: None,
        };

        let resolved = OciResolver::new()
            .resolve(&source, Arch::Amd64, &image_dir)
            .await
            .unwrap();

        let expected: [u8; 32] = Sha256::digest(content).into();
        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                sha256: expected,
                size: content.len() as u64,
            }
        );
    }
}
