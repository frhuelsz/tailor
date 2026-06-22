use tailor_config::ToolchainEntry;
use tailor_core::ResolveError;

use crate::oci;

const DIGEST_SEPARATOR: char = '@';
const PATH_SEPARATOR: char = '/';
const TAG_SEPARATOR: char = ':';

pub(crate) async fn resolve(toolchain: &ToolchainEntry) -> Result<String, ResolveError> {
    let reference = toolchain_reference(toolchain);

    oci::resolve_reference_digest(reference).await
}

fn toolchain_reference(toolchain: &ToolchainEntry) -> String {
    if has_tag_or_digest(&toolchain.container) {
        return toolchain.container.clone();
    }

    let container = &toolchain.container;
    format!("{container}:{}", toolchain.effective_tag())
}

fn has_tag_or_digest(reference: &str) -> bool {
    reference.contains(DIGEST_SEPARATOR)
        || reference
            .rsplit(PATH_SEPARATOR)
            .next()
            .is_some_and(|last_segment| last_segment.contains(TAG_SEPARATOR))
}

#[cfg(test)]
mod tests {
    use super::*;

    use semver::Version;

    fn toolchain(tag: Option<String>) -> ToolchainEntry {
        ToolchainEntry {
            container: "mcr.microsoft.com/azurelinux/imagecustomizer".to_owned(),
            version: Some(Version::parse("1.3.0").unwrap()),
            tag,
        }
    }

    #[test]
    fn defaults_tag_to_semver_without_v() {
        assert_eq!(
            toolchain_reference(&toolchain(None)),
            "mcr.microsoft.com/azurelinux/imagecustomizer:1.3.0"
        );
    }

    #[test]
    fn defaults_to_latest_when_no_tag_or_version() {
        let entry = ToolchainEntry {
            container: "mcr.microsoft.com/azurelinux/imagecustomizer".to_owned(),
            version: None,
            tag: None,
        };
        assert_eq!(
            toolchain_reference(&entry),
            "mcr.microsoft.com/azurelinux/imagecustomizer:latest"
        );
    }

    #[test]
    fn uses_explicit_tag() {
        assert_eq!(
            toolchain_reference(&toolchain(Some("latest".to_owned()))),
            "mcr.microsoft.com/azurelinux/imagecustomizer:latest"
        );
    }

    #[test]
    fn preserves_existing_tag_or_digest() {
        let mut tagged = toolchain(None);
        tagged.container = "localhost:5000/repo/image:custom".to_owned();
        assert_eq!(toolchain_reference(&tagged), tagged.container);

        let mut digested = toolchain(None);
        digested.container = "example.com/repo/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
        assert_eq!(toolchain_reference(&digested), digested.container);
    }

    #[tokio::test]
    #[ignore = "live registry test; requires network access and MCR image availability"]
    async fn resolves_toolchain_digest() {
        let digest = resolve(&toolchain(Some("latest".to_owned())))
            .await
            .unwrap();

        assert!(digest.starts_with("sha256:"));
    }
}
