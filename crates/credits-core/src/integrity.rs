use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CoreError, Result};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceFile {
    pub path: String,
    pub sha256: String,
    pub size_bytes: u64,
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| CoreError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn evidence_file(root: &Path, path: &Path) -> Result<EvidenceFile> {
    let relative = path.strip_prefix(root).map_err(|_| {
        CoreError::Integrity(format!(
            "evidence path {} is outside {}",
            path.display(),
            root.display()
        ))
    })?;
    let size_bytes = std::fs::metadata(path)
        .map_err(|source| CoreError::Read {
            path: path.to_path_buf(),
            source,
        })?
        .len();
    Ok(EvidenceFile {
        path: normalize_relative(relative),
        sha256: sha256_file(path)?,
        size_bytes,
    })
}

pub fn write_checksum_sidecar(target: &Path, checksum: &str) -> Result<()> {
    let sidecar = PathBuf::from(format!("{}.sha256", target.display()));
    let mut file = File::create(&sidecar).map_err(|source| CoreError::Write {
        path: sidecar.clone(),
        source,
    })?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("evidence");
    writeln!(file, "{checksum}  {name}").map_err(|source| CoreError::Write {
        path: sidecar,
        source,
    })
}

pub fn verify_checksum_sidecar(target: &Path) -> Result<String> {
    let sidecar = PathBuf::from(format!("{}.sha256", target.display()));
    let file = File::open(&sidecar).map_err(|source| CoreError::Read {
        path: sidecar.clone(),
        source,
    })?;
    let mut line = String::new();
    BufReader::new(file)
        .read_line(&mut line)
        .map_err(|source| CoreError::Read {
            path: sidecar,
            source,
        })?;
    let expected = line
        .split_whitespace()
        .next()
        .ok_or_else(|| CoreError::Integrity("empty checksum sidecar".to_string()))?;
    let actual = sha256_file(target)?;
    if expected != actual {
        return Err(CoreError::Integrity(format!(
            "{} checksum mismatch: expected {expected}, got {actual}",
            target.display()
        )));
    }
    Ok(actual)
}

pub fn ensure_safe_relative(path: &str) -> Result<PathBuf> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Err(CoreError::Integrity(format!(
            "unsafe evidence path: {path}"
        )));
    }
    Ok(relative.to_path_buf())
}

fn normalize_relative(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_bytes() {
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn rejects_parent_paths() {
        assert!(ensure_safe_relative("evidence/ok.png").is_ok());
        assert!(ensure_safe_relative("../escape.png").is_err());
        assert!(ensure_safe_relative("/tmp/escape.png").is_err());
    }
}
