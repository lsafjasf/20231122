//! SHA-256 helpers. Every piece of evidence (media file, thumbnail, manifest)
//! is identified and verified by a lowercase hex SHA-256 digest so a reviewer
//! can independently reproduce it with standard system tools.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// Stream-hash a file in fixed-size chunks without loading it into memory.
pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
    let path = path.as_ref();
    let file = File::open(path).map_err(|e| Error::io(path, e))?;
    let mut reader = BufReader::with_capacity(1 << 20, file);
    let mut hasher = Sha256::new();
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let read = reader.read(&mut chunk).map_err(|e| Error::io(path, e))?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Hash an in-memory byte buffer (used for thumbnail evidence).
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Constant-time-ish digest comparison after normalizing casing/whitespace.
pub fn digest_equal(left: &str, right: &str) -> bool {
    let norm = |value: &str| value.trim().to_ascii_lowercase();
    norm(left) == norm(right)
}
