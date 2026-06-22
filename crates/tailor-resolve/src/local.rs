use std::path::Path;

use sha2::{Digest, Sha256};
use tokio::{fs::File, io::AsyncReadExt};

use tailor_core::{ResolveError, ResolvedBase};

const HASH_BUFFER_SIZE: usize = 64 * 1024;

pub(crate) async fn resolve(path: impl AsRef<Path>) -> Result<ResolvedBase, ResolveError> {
    let path = path.as_ref();
    let mut file = File::open(path)
        .await
        .map_err(|source| ResolveError::LocalRead {
            path: path.to_path_buf(),
            source,
        })?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];

    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|source| ResolveError::LocalRead {
                path: path.to_path_buf(),
                source,
            })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }

    Ok(ResolvedBase::LocalFile {
        sha256: hasher.finalize().into(),
        size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::tempdir;

    #[tokio::test]
    async fn hashes_local_file_and_reports_size() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("base.raw");
        let content = b"tailor local base\n";
        fs::write(&path, content).unwrap();

        let resolved = resolve(&path).await.unwrap();

        let expected: [u8; 32] = Sha256::digest(content).into();
        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                sha256: expected,
                size: content.len() as u64,
            }
        );
    }

    #[tokio::test]
    async fn reports_local_read_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.raw");

        let err = resolve(&path).await.unwrap_err();

        assert!(matches!(err, ResolveError::LocalRead { .. }), "got {err:?}");
    }
}
