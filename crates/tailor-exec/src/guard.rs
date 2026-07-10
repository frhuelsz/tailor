use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Component, Path, PathBuf},
};

use tailor_core::ExecError;

const ROOT_PATH: &str = "/";

pub(crate) fn ensure_safe_build_dir(path: &Path) -> Result<(), ExecError> {
    ensure_safe_dir(path, true)
}

pub(crate) fn ensure_safe_rw_target(path: &Path) -> Result<(), ExecError> {
    ensure_safe_dir(path, false)
}

fn ensure_safe_dir(path: &Path, require_separate_device: bool) -> Result<(), ExecError> {
    let normalized = normalize_absolute_lexical(path)?;
    let root = Path::new(ROOT_PATH);
    if normalized == root {
        return Err(unsafe_dir(
            normalized,
            "must not be the filesystem root".to_owned(),
        ));
    }

    let cwd =
        normalize_absolute_lexical(&std::env::current_dir().map_err(|source| ExecError::Io {
            context: "failed to determine current directory".to_owned(),
            source,
        })?)?;
    if cwd.starts_with(&normalized) {
        return Err(unsafe_dir(
            normalized,
            format!(
                "must not contain the current working directory `{}`",
                cwd.display()
            ),
        ));
    }

    if require_separate_device {
        let root_dev = fs::metadata(root)
            .map_err(|source| ExecError::Io {
                context: "failed to stat filesystem root `/`".to_owned(),
                source,
            })?
            .dev();
        let ancestor = nearest_existing_ancestor(&normalized)?;
        let ancestor_dev = fs::metadata(&ancestor)
            .map_err(|source| ExecError::Io {
                context: format!("failed to stat `{}`", ancestor.display()),
                source,
            })?
            .dev();
        if ancestor_dev == root_dev {
            return Err(unsafe_dir(
                normalized,
                format!(
                    "nearest existing ancestor `{}` is on the same device as `/`",
                    ancestor.display()
                ),
            ));
        }
    }

    Ok(())
}

fn unsafe_dir(path: PathBuf, reason: String) -> ExecError {
    ExecError::UnsafeDir { path, reason }
}

fn nearest_existing_ancestor(path: &Path) -> Result<PathBuf, ExecError> {
    let mut candidate = path.to_path_buf();
    loop {
        match fs::metadata(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if !candidate.pop() {
                    return Ok(PathBuf::from(ROOT_PATH));
                }
            }
            Err(source) => {
                return Err(ExecError::Io {
                    context: format!("failed to stat `{}`", candidate.display()),
                    source,
                });
            }
        }
    }
}

fn normalize_absolute_lexical(path: &Path) -> Result<PathBuf, ExecError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| ExecError::Io {
                context: "failed to determine current directory".to_owned(),
                source,
            })?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(ROOT_PATH)),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized != Path::new(ROOT_PATH) {
                    normalized.pop();
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        Ok(PathBuf::from(ROOT_PATH))
    } else {
        Ok(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same_device(left: &Path, right: &Path) -> bool {
        fs::metadata(left).unwrap().dev() == fs::metadata(right).unwrap().dev()
    }

    #[test]
    fn rejects_filesystem_root() {
        let err = ensure_safe_build_dir(Path::new(ROOT_PATH)).unwrap_err();
        assert!(matches!(err, ExecError::UnsafeDir { .. }));
    }

    #[test]
    fn rejects_ancestor_of_current_working_dir() {
        let cwd = std::env::current_dir().unwrap();

        let err = ensure_safe_rw_target(&cwd).unwrap_err();

        assert!(matches!(err, ExecError::UnsafeDir { .. }));
    }

    #[test]
    fn same_device_dir_is_rejected_for_build_but_allowed_for_rw_target() {
        let temp = tempfile::Builder::new()
            .prefix("tailor-guard-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        if !same_device(temp.path(), Path::new(ROOT_PATH)) {
            return;
        }

        let err = ensure_safe_build_dir(temp.path()).unwrap_err();
        assert!(matches!(err, ExecError::UnsafeDir { .. }));
        ensure_safe_rw_target(temp.path()).unwrap();
    }

    #[test]
    fn separate_filesystem_build_dir_is_allowed() {
        let candidate = Path::new("/dev/shm/tailor-build-dir");
        if !candidate
            .parent()
            .is_some_and(|parent| parent.exists() && !same_device(parent, Path::new(ROOT_PATH)))
        {
            return;
        }

        ensure_safe_build_dir(candidate).unwrap();
    }

    #[test]
    fn normalizes_without_requiring_leaf_to_exist() {
        let candidate = Path::new("/dev/shm/../shm/tailor-build-dir");
        if !Path::new("/dev/shm").exists()
            || same_device(Path::new("/dev/shm"), Path::new(ROOT_PATH))
        {
            return;
        }

        ensure_safe_build_dir(candidate).unwrap();
    }
}
