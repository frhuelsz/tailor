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
    async fn resolve(&self, source: &BaseSource, arch: Arch) -> Result<ResolvedBase, ResolveError> {
        match source {
            BaseSource::Path { path } => local::resolve(path).await,
            BaseSource::Oci { oci } => oci::resolve(oci, arch).await,
            BaseSource::AzureLinux { azure_linux } => azure_linux::resolve(azure_linux, arch).await,
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
        let source = BaseSource::Path { path };

        let resolved = OciResolver::new()
            .resolve(&source, Arch::Amd64)
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
