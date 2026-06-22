//! The canonical per-cell fingerprint — a SHA-256 over every build-affecting input (`meta/docs/design.md`
//! §9.1). Deterministic given a deterministic render, so it is stable across machines and runs.

use serde_yaml_ng::Value;
use sha2::{Digest, Sha256};
use tailor_config::Operation;

use crate::{domain::Fingerprint, ports::ResolvedBase};

/// All inputs that determine a cell's output. Registry digests come from resolution/the lock; local
/// hashes are computed at build time.
pub struct FingerprintInputs<'a> {
    pub slug: &'a str,
    pub toolchain_digest: &'a str,
    pub base: &'a ResolvedBase,
    pub ic_config: &'a Value,
    pub operation: Operation,
    pub inject_files: bool,
    /// Sorted SHA-256 hashes of `extraDependencies` files.
    pub extra_dependency_hashes: &'a [[u8; 32]],
    /// Sorted SHA-256 hashes of `rpmSources` contents (excluding `repodata/`).
    pub rpm_source_hashes: &'a [[u8; 32]],
}

/// Compute the canonical fingerprint. Each field is domain-separated and length-prefixed so distinct
/// inputs can never collide by concatenation.
pub fn fingerprint(inputs: &FingerprintInputs<'_>) -> Fingerprint {
    let mut hasher = Sha256::new();

    field(&mut hasher, b"slug", inputs.slug.as_bytes());
    field(
        &mut hasher,
        b"toolchain",
        inputs.toolchain_digest.as_bytes(),
    );
    match inputs.base {
        ResolvedBase::LocalFile { sha256, size } => {
            field(&mut hasher, b"base.local", sha256);
            field(&mut hasher, b"base.size", &size.to_le_bytes());
        }
        ResolvedBase::Oci {
            reference,
            platform,
            digest,
        } => {
            field(&mut hasher, b"base.oci.ref", reference.as_bytes());
            field(&mut hasher, b"base.oci.platform", platform.as_bytes());
            field(&mut hasher, b"base.oci.digest", digest.as_bytes());
        }
    }
    field(&mut hasher, b"config", &canonical_config(inputs.ic_config));
    field(&mut hasher, b"operation", operation_tag(inputs.operation));
    field(&mut hasher, b"inject", &[u8::from(inputs.inject_files)]);
    for hash in inputs.extra_dependency_hashes {
        field(&mut hasher, b"dep", hash);
    }
    for hash in inputs.rpm_source_hashes {
        field(&mut hasher, b"rpm", hash);
    }

    Fingerprint(hasher.finalize().into())
}

/// A deterministic byte form of the merged config (the rendered config is already deterministic).
pub fn canonical_config(config: &Value) -> Vec<u8> {
    serde_yaml_ng::to_string(config)
        .unwrap_or_default()
        .into_bytes()
}

fn operation_tag(operation: Operation) -> &'static [u8] {
    match operation {
        Operation::Customize => b"customize",
        Operation::Convert => b"convert",
    }
}

fn field(hasher: &mut Sha256, label: &[u8], bytes: &[u8]) {
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label);
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> ResolvedBase {
        ResolvedBase::LocalFile {
            sha256: [1; 32],
            size: 100,
        }
    }

    fn inputs<'a>(
        slug: &'a str,
        config: &'a Value,
        base: &'a ResolvedBase,
    ) -> FingerprintInputs<'a> {
        FingerprintInputs {
            slug,
            toolchain_digest: "sha256:abc",
            base,
            ic_config: config,
            operation: Operation::Customize,
            inject_files: false,
            extra_dependency_hashes: &[],
            rpm_source_hashes: &[],
        }
    }

    #[test]
    fn fingerprint_is_deterministic() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let a = fingerprint(&inputs("cell", &cfg, &base));
        let b = fingerprint(&inputs("cell", &cfg, &base));
        assert_eq!(a, b);
    }

    #[test]
    fn config_change_changes_fingerprint() {
        let base = base();
        let cfg_a: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        let cfg_b: Value = serde_yaml_ng::from_str("os:\n  hostname: b\n").unwrap();
        assert_ne!(
            fingerprint(&inputs("cell", &cfg_a, &base)),
            fingerprint(&inputs("cell", &cfg_b, &base))
        );
    }

    #[test]
    fn slug_is_part_of_the_fingerprint() {
        let base = base();
        let cfg: Value = serde_yaml_ng::from_str("os:\n  hostname: a\n").unwrap();
        assert_ne!(
            fingerprint(&inputs("cell-a", &cfg, &base)),
            fingerprint(&inputs("cell-b", &cfg, &base))
        );
    }
}
