//! xxhash64 for on-disk audio files. Streaming read with a 64 KB buffer
//! to avoid pulling multi-GB lossless files fully into memory.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use twox_hash::XxHash64;

#[derive(Debug, thiserror::Error)]
pub enum HashError {
    #[error("io error on {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// xxhash64 of the file at `path`. Seed 0 (matches `twox-hash`'s default).
pub fn hash_file(path: &Path) -> Result<u64, HashError> {
    use std::hash::Hasher;

    let file = File::open(path).map_err(|source| HashError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = XxHash64::with_seed(0);
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|source| HashError::Io {
            path: path.display().to_string(),
            source,
        })?;
        if n == 0 {
            break;
        }
        hasher.write(&buf[..n]);
    }
    Ok(hasher.finish())
}

/// xxhash64 encoded as zero-padded 16-char lowercase hex — the canonical
/// on-disk form in the `file_hash` TEXT column.
pub fn hash_hex(h: u64) -> String {
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn empty_file_hash_is_stable() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let h = hash_file(tmp.path()).unwrap();
        let h2 = hash_file(tmp.path()).unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn different_content_hashes_differ() {
        let mut a = tempfile::NamedTempFile::new().unwrap();
        a.write_all(b"alpha").unwrap();
        let mut b = tempfile::NamedTempFile::new().unwrap();
        b.write_all(b"bravo").unwrap();
        assert_ne!(hash_file(a.path()).unwrap(), hash_file(b.path()).unwrap());
    }

    #[test]
    fn large_content_streams_without_oom() {
        let mut t = tempfile::NamedTempFile::new().unwrap();
        let chunk = vec![0u8; 1024 * 1024];
        for _ in 0..10 {
            t.write_all(&chunk).unwrap();
        }
        let h = hash_file(t.path()).unwrap();
        assert_ne!(h, 0);
    }

    #[test]
    fn missing_file_returns_io_error() {
        let err = hash_file(Path::new("/no/such/path")).unwrap_err();
        assert!(matches!(err, HashError::Io { .. }));
    }

    #[test]
    fn hex_is_16_chars_lowercase() {
        assert_eq!(hash_hex(0).len(), 16);
        assert_eq!(hash_hex(0xDEADBEEF_DEADBEEF), "deadbeefdeadbeef");
    }
}
