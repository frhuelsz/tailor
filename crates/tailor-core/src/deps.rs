//! Content hashing of a cell's **declared local dependencies** — the `extraDependencies` and
//! `rpmSources` paths named in `image.yaml`. IC-config-referenced assets (an `additionalFiles`
//! script, a unit file, a local RPM) are only *named* in the merged config, so hashing the config
//! text alone leaves an edit to one of those files invisible to the incremental fingerprint
//! (`meta/docs/2026-06-22-design.md` §9, `crates/tailor-core/src/fingerprint.rs`). This module walks
//! each declared path and produces the sorted per-file SHA-256 hashes the fingerprint folds in.
//!
//! Content hashing (not mtime/size) is deliberate: a directory's mtime does not change when a file
//! inside it is edited in place, so a metadata probe would miss exactly the case `extraDependencies`
//! exists to catch.

use std::{
    fs,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::error::CoreError;

/// A directory basename excluded from `rpmSources` hashing: it holds regenerated repo metadata whose
/// bytes are derived from the RPMs themselves, so hashing it adds churn without adding signal
/// (`fingerprint.rs` `rpm_source_hashes` — "excluding `repodata/`").
pub(crate) const REPODATA_DIR: &str = "repodata";

/// Compute the sorted per-file SHA-256 hashes for a set of declared dependency `paths` (each a file
/// or a directory), resolved relative to `base_dir`. Each regular file contributes one hash over its
/// path (relative to `base_dir`) **and** its content, so both in-place edits and renames change the
/// fingerprint; the returned list is sorted so `readdir` order never affects it. `exclude_dir`, when
/// set, skips any directory with that basename anywhere in the walk (e.g. `repodata`).
///
/// A declared path that does not exist is a hard error (fail-closed: a signed/reproducible build must
/// not silently treat a missing dependency as "no change").
pub(crate) fn hash_dependencies(
    paths: &[PathBuf],
    base_dir: &Path,
    exclude_dir: Option<&str>,
    image: &str,
) -> Result<Vec<[u8; 32]>, CoreError> {
    let mut hashes = Vec::new();
    for path in paths {
        let absolute = tailor_config::absolutize(path, base_dir);
        let metadata =
            fs::symlink_metadata(&absolute).map_err(|_| CoreError::MissingDependency {
                image: image.to_owned(),
                path: path.clone(),
            })?;
        if metadata.is_dir() {
            walk_dir(&absolute, base_dir, exclude_dir, &mut hashes)?;
        } else if metadata.is_file() {
            hashes.push(hash_one(&absolute, base_dir)?);
        }
        // Symlinks and other special files are skipped: their target content is hashed if it is
        // itself a declared path, and following them risks cycles.
    }
    hashes.sort_unstable();
    Ok(hashes)
}

/// Recursively hash every regular file under `dir`, skipping any subdirectory named `exclude_dir`.
fn walk_dir(
    dir: &Path,
    base_dir: &Path,
    exclude_dir: Option<&str>,
    hashes: &mut Vec<[u8; 32]>,
) -> Result<(), CoreError> {
    let entries = fs::read_dir(dir).map_err(|source| {
        CoreError::Exec(crate::error::ExecError::Io {
            context: format!("failed to read dependency directory `{}`", dir.display()),
            source,
        })
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            CoreError::Exec(crate::error::ExecError::Io {
                context: format!(
                    "failed to read a dependency entry under `{}`",
                    dir.display()
                ),
                source,
            })
        })?;
        let file_type = entry.file_type().map_err(|source| {
            CoreError::Exec(crate::error::ExecError::Io {
                context: format!("failed to stat `{}`", entry.path().display()),
                source,
            })
        })?;
        let path = entry.path();
        if file_type.is_dir() {
            if exclude_dir.is_some_and(|name| entry.file_name() == *name) {
                continue;
            }
            walk_dir(&path, base_dir, exclude_dir, hashes)?;
        } else if file_type.is_file() {
            hashes.push(hash_one(&path, base_dir)?);
        }
    }
    Ok(())
}

/// SHA-256 over the file's path (relative to `base_dir` when possible, for rename sensitivity and
/// machine portability) domain-separated from its content.
fn hash_one(path: &Path, base_dir: &Path) -> Result<[u8; 32], CoreError> {
    let content = fs::read(path).map_err(|source| {
        CoreError::Exec(crate::error::ExecError::Io {
            context: format!("failed to read dependency file `{}`", path.display()),
            source,
        })
    })?;
    let rel = path.strip_prefix(base_dir).unwrap_or(path);
    let mut hasher = Sha256::new();
    let rel_bytes = rel.to_string_lossy();
    hasher.update((rel_bytes.len() as u64).to_le_bytes());
    hasher.update(rel_bytes.as_bytes());
    hasher.update((content.len() as u64).to_le_bytes());
    hasher.update(&content);
    Ok(hasher.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn hashes_are_stable_and_order_independent() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "files/a.sh", "one");
        write(dir.path(), "files/nested/b.txt", "two");
        let paths = vec![PathBuf::from("files")];
        let first = hash_dependencies(&paths, dir.path(), None, "img").unwrap();
        let second = hash_dependencies(&paths, dir.path(), None, "img").unwrap();
        assert_eq!(first, second, "hashing must be deterministic");
        assert_eq!(first.len(), 2, "both files contribute a hash");
    }

    #[test]
    fn an_in_place_edit_changes_the_hash_set() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "files/a.sh", "before");
        let paths = vec![PathBuf::from("files")];
        let before = hash_dependencies(&paths, dir.path(), None, "img").unwrap();
        write(dir.path(), "files/a.sh", "after"); // same path, new content
        let after = hash_dependencies(&paths, dir.path(), None, "img").unwrap();
        assert_ne!(
            before, after,
            "an in-place content edit must change the hashes"
        );
    }

    #[test]
    fn a_rename_changes_the_hash_set() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "files/a.sh", "body");
        let paths = vec![PathBuf::from("files")];
        let before = hash_dependencies(&paths, dir.path(), None, "img").unwrap();
        fs::rename(dir.path().join("files/a.sh"), dir.path().join("files/b.sh")).unwrap();
        let after = hash_dependencies(&paths, dir.path(), None, "img").unwrap();
        assert_ne!(
            before, after,
            "a rename (path change) must change the hashes"
        );
    }

    #[test]
    fn repodata_is_excluded_for_rpm_sources() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "rpms/pkg.rpm", "rpm");
        write(dir.path(), "rpms/repodata/primary.xml", "meta");
        let paths = vec![PathBuf::from("rpms")];
        let hashes = hash_dependencies(&paths, dir.path(), Some(REPODATA_DIR), "img").unwrap();
        assert_eq!(
            hashes.len(),
            1,
            "only the rpm is hashed, repodata is skipped"
        );
    }

    #[test]
    fn a_missing_declared_path_is_an_error() {
        let dir = TempDir::new().unwrap();
        let paths = vec![PathBuf::from("does-not-exist")];
        let err = hash_dependencies(&paths, dir.path(), None, "img").unwrap_err();
        assert!(matches!(err, CoreError::MissingDependency { .. }));
    }

    #[test]
    fn a_single_declared_file_is_hashed() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "solo.sh", "hi");
        let paths = vec![PathBuf::from("solo.sh")];
        let hashes = hash_dependencies(&paths, dir.path(), None, "img").unwrap();
        assert_eq!(hashes.len(), 1);
    }
}
