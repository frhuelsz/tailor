use std::{
    env,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use tailor_core::{ResolveError, ResolvedBase};
use xxhash_rust::xxh3::{self, Xxh3};

const HASH_BUFFER_SIZE: usize = 8 * 1024 * 1024;
const CACHE_VERSION: &str = "1";
const CACHE_FIELD_SEPARATOR: &str = " | ";
const CACHE_ENTRY_EXTENSION: &str = "txt";
const CONTENT_HASH_BYTES: usize = 16;

pub(crate) async fn resolve(
    path: impl AsRef<Path>,
    cache_dir: Option<&Path>,
) -> Result<ResolvedBase, ResolveError> {
    let path = path.as_ref().to_path_buf();
    let cache_dir = cache_dir.map(Path::to_path_buf);
    tokio::task::spawn_blocking(move || resolve_blocking(&path, cache_dir.as_deref()))
        .await
        .map_err(|source| ResolveError::Other(format!("local base hash task failed: {source}")))?
}

fn resolve_blocking(path: &Path, cache_dir: Option<&Path>) -> Result<ResolvedBase, ResolveError> {
    let metadata = fs::metadata(path).map_err(|source| read_err(path, source))?;
    let metadata_size = metadata.len();
    let mtime_ns = modified_time_ns(&metadata);
    let abs_path = absolute_path_string(path);

    if let (Some(dir), Some(mtime_ns)) = (cache_dir, mtime_ns)
        && let Some(content_hash) = read_cache_entry(dir, metadata_size, mtime_ns, &abs_path)
    {
        return Ok(ResolvedBase::LocalFile {
            content_hash,
            size: metadata_size,
        });
    }

    let (content_hash, size) = hash_file(path)?;
    if let (Some(dir), Some(mtime_ns)) = (cache_dir, mtime_ns) {
        let _ = write_cache_entry(dir, size, mtime_ns, &content_hash, &abs_path);
    }

    Ok(ResolvedBase::LocalFile { content_hash, size })
}

fn hash_file(path: &Path) -> Result<([u8; CONTENT_HASH_BYTES], u64), ResolveError> {
    let mut file = File::open(path).map_err(|source| read_err(path, source))?;
    let mut hasher = Xxh3::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|source| read_err(path, source))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }

    Ok((hasher.digest128().to_le_bytes(), size))
}

fn read_err(path: &Path, source: io::Error) -> ResolveError {
    ResolveError::LocalRead {
        path: path.to_path_buf(),
        source,
    }
}

fn modified_time_ns(metadata: &fs::Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn absolute_path_string(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    absolute.to_string_lossy().into_owned()
}

fn read_cache_entry(
    cache_dir: &Path,
    size: u64,
    mtime_ns: u128,
    abs_path: &str,
) -> Option<[u8; CONTENT_HASH_BYTES]> {
    let text = fs::read_to_string(cache_entry_path(cache_dir, abs_path)).ok()?;
    let line = text.trim_end_matches(['\n', '\r']);
    let mut fields = line.splitn(5, CACHE_FIELD_SEPARATOR);
    let version = fields.next()?;
    let stored_size = fields.next()?.parse::<u64>().ok()?;
    let stored_mtime_ns = fields.next()?.parse::<u128>().ok()?;
    let hash_hex = fields.next()?;
    let stored_path = fields.next()?;
    if version != CACHE_VERSION
        || stored_size != size
        || stored_mtime_ns != mtime_ns
        || stored_path != abs_path
    {
        return None;
    }

    let mut hash = [0_u8; CONTENT_HASH_BYTES];
    hex::decode_to_slice(hash_hex, &mut hash).ok()?;
    Some(hash)
}

fn write_cache_entry(
    cache_dir: &Path,
    size: u64,
    mtime_ns: u128,
    content_hash: &[u8; CONTENT_HASH_BYTES],
    abs_path: &str,
) -> Result<(), io::Error> {
    if abs_path.contains('\n') || abs_path.contains('\r') {
        return Ok(());
    }
    fs::create_dir_all(cache_dir)?;
    let hash_hex = hex::encode(content_hash);
    let line = format!(
        "{CACHE_VERSION}{CACHE_FIELD_SEPARATOR}{size}{CACHE_FIELD_SEPARATOR}{mtime_ns}{CACHE_FIELD_SEPARATOR}{hash_hex}{CACHE_FIELD_SEPARATOR}{abs_path}\n"
    );
    fs::write(cache_entry_path(cache_dir, abs_path), line)
}

fn cache_entry_path(cache_dir: &Path, abs_path: &str) -> PathBuf {
    let key = hex::encode(xxh3::xxh3_128(abs_path.as_bytes()).to_le_bytes());
    cache_dir.join(format!("{key}.{CACHE_ENTRY_EXTENSION}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::tempdir;
    use xxhash_rust::xxh3;

    #[tokio::test]
    async fn hashes_local_file_and_reports_size() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("base.raw");
        let content = b"tailor local base\n";
        fs::write(&path, content).unwrap();

        let resolved = resolve(&path, None).await.unwrap();

        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected_hash(content),
                size: content.len() as u64,
            }
        );
    }

    #[tokio::test]
    async fn reports_local_read_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.raw");

        let err = resolve(&path, None).await.unwrap_err();

        assert!(matches!(err, ResolveError::LocalRead { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn cache_hit_reuses_stored_hash() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let path = dir.path().join("base.raw");
        let content = b"real content";
        let stored_hash = [9_u8; CONTENT_HASH_BYTES];
        fs::write(&path, content).unwrap();
        write_matching_entry(&cache_dir, &path, &stored_hash, None, None);

        let resolved = resolve(&path, Some(&cache_dir)).await.unwrap();

        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: stored_hash,
                size: content.len() as u64,
            }
        );
        assert_ne!(stored_hash, expected_hash(content));
    }

    #[tokio::test]
    async fn mtime_change_misses_cache_and_rehashes() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let path = dir.path().join("base.raw");
        let content = b"real content";
        fs::write(&path, content).unwrap();
        write_matching_entry(
            &cache_dir,
            &path,
            &[9_u8; CONTENT_HASH_BYTES],
            None,
            Some(0),
        );

        let resolved = resolve(&path, Some(&cache_dir)).await.unwrap();

        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected_hash(content),
                size: content.len() as u64,
            }
        );
    }

    #[tokio::test]
    async fn size_change_misses_cache_and_rehashes() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let path = dir.path().join("base.raw");
        let content = b"real content";
        fs::write(&path, content).unwrap();
        write_matching_entry(
            &cache_dir,
            &path,
            &[9_u8; CONTENT_HASH_BYTES],
            Some(u64::MAX),
            None,
        );

        let resolved = resolve(&path, Some(&cache_dir)).await.unwrap();

        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected_hash(content),
                size: content.len() as u64,
            }
        );
    }

    #[tokio::test]
    async fn no_cache_dir_hashes_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("base.raw");
        let content = b"uncached content";
        fs::write(&path, content).unwrap();

        let resolved = resolve(&path, None).await.unwrap();

        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected_hash(content),
                size: content.len() as u64,
            }
        );
    }

    #[tokio::test]
    async fn unusable_cache_dir_falls_back_to_hashing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("base.raw");
        let cache_dir = dir.path().join("cache-as-file");
        let content = b"uncached content";
        fs::write(&path, content).unwrap();
        fs::write(&cache_dir, b"not a directory").unwrap();

        let resolved = resolve(&path, Some(&cache_dir)).await.unwrap();

        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected_hash(content),
                size: content.len() as u64,
            }
        );
    }

    fn expected_hash(content: &[u8]) -> [u8; CONTENT_HASH_BYTES] {
        xxh3::xxh3_128(content).to_le_bytes()
    }

    fn write_matching_entry(
        cache_dir: &Path,
        path: &Path,
        content_hash: &[u8; CONTENT_HASH_BYTES],
        size_override: Option<u64>,
        mtime_override: Option<u128>,
    ) {
        let metadata = fs::metadata(path).unwrap();
        let size = size_override.unwrap_or(metadata.len());
        let mtime_ns = mtime_override.unwrap_or_else(|| modified_time_ns(&metadata).unwrap());
        write_cache_entry(
            cache_dir,
            size,
            mtime_ns,
            content_hash,
            &absolute_path_string(path),
        )
        .unwrap();
    }
}
