use std::path::{Component, Path, PathBuf};

const DEFAULT_HOST_ROOT: &str = "/host";

pub fn to_container_path(host_path: &Path, host_root: &Path) -> String {
    let root = if host_root.as_os_str().is_empty() {
        Path::new(DEFAULT_HOST_ROOT)
    } else {
        host_root
    };
    let suffix = host_path.strip_prefix(Path::new("/")).unwrap_or(host_path);
    let mut translated = PathBuf::from(root);
    translated.push(suffix);
    translated.to_string_lossy().into_owned()
}

/// Resolve `path` to an absolute, lexically-normalized host path: a relative path (as authored in
/// `image.yaml`, e.g. a `base` or `rpmSources` entry) is joined onto `base_dir` (the image
/// directory), then `.`/`..` components are collapsed — so it translates to a clean `/host/<abs>`.
pub fn absolutize(path: &Path, base_dir: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    };
    normalize(&joined)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !matches!(
                    out.components().next_back(),
                    Some(Component::RootDir) | None
                ) {
                    out.pop();
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    #[test]
    fn translates_absolute_path_under_host_root() {
        assert_eq!(
            to_container_path(Path::new("/home/me/image.vhdx"), Path::new("/host")),
            "/host/home/me/image.vhdx"
        );
    }

    #[test]
    fn empty_host_root_uses_default() {
        assert_eq!(
            to_container_path(Path::new("/var/lib/x"), Path::new("")),
            "/host/var/lib/x"
        );
    }

    #[test]
    fn custom_host_root_is_respected() {
        assert_eq!(
            to_container_path(Path::new("/work/out.raw"), Path::new("/mnt/host")),
            "/mnt/host/work/out.raw"
        );
    }

    #[test]
    fn absolutize_resolves_relative_base_against_the_image_dir() {
        // Two levels up from `/repo/docs/img` lands in `/repo`, collapsing cleanly.
        let resolved = absolutize(
            Path::new("../../artifacts/core.vhdx"),
            Path::new("/repo/docs/img"),
        );
        assert_eq!(resolved, Path::new("/repo/artifacts/core.vhdx"));
        assert_eq!(
            to_container_path(&resolved, Path::new("/host")),
            "/host/repo/artifacts/core.vhdx"
        );
    }

    #[test]
    fn absolutize_leaves_absolute_paths_normalized() {
        assert_eq!(
            absolutize(Path::new("/abs/base.vhdx"), Path::new("/images")),
            Path::new("/abs/base.vhdx")
        );
    }
}
