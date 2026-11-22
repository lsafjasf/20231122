//! Discovery of audio objects, including files whose extension is unknown or
//! missing. Such files are recovered via content sniffing (magic bytes).

use crate::error::{DownmixError, Result};
use crate::hash::sha256_file;
use crate::model::{AudioObject, DetectionKind};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Extensions recognized without content sniffing.
pub const KNOWN_EXTENSIONS: &[&str] = &[
    "wav", "wave", "flac", "ogg", "oga", "opus", "mp3", "m4a", "aac", "wma", "aiff", "aif",
    "mka", "webm", "mp4", "mov",
];

/// Magic-byte signatures that identify a readable audio container.
const SIGNATURES: &[(&[u8], &str)] = &[
    (b"RIFF", "wav"),
    (b"OggS", "ogg"),
    (b"fLaC", "flac"),
    (b"ID3", "mp3"),
    (b"\xff\xfb", "mp3"),
    (b"\xff\xf3", "mp3"),
    (b"\xff\xf2", "mp3"),
    (b"ftyp", "m4a"),
];

/// Stable per-path identifier used as evidence directory name.
pub fn object_id_from_path(path: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hex::encode(hasher.finalize())[..16].to_string()
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Read up to the first 12 bytes and try to identify an audio container.
fn sniff(path: &Path) -> Result<Option<String>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|e| DownmixError::io(path, e))?;
    let mut head = [0u8; 12];
    let n = file
        .read(&mut head)
        .map_err(|e| DownmixError::io(path, e))?;
    let head = &head[..n];
    for (magic, ext) in SIGNATURES {
        if head.len() >= magic.len() && &head[..magic.len()] == *magic {
            return Ok(Some((*ext).to_string()));
        }
    }
    Ok(None)
}

/// Discover all candidate audio objects under `root`, recursively.
///
/// Files with known extensions are accepted directly. Files with unknown or
/// missing extensions are content-sniffed; readable ones are marked
/// [`DetectionKind::SniffedContent`], everything else is ignored.
pub fn discover_objects(root: &Path) -> Result<Vec<AudioObject>> {
    let mut objects = Vec::new();
    for entry in WalkDir::new(root).follow_links(false).into_iter() {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                log::warn!("walk error under {root:?}: {err}");
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = extension_of(path);
        let known = KNOWN_EXTENSIONS.contains(&ext.as_str());
        let sniffed = if known {
            None
        } else {
            match sniff(path) {
                Ok(hit) => hit,
                Err(err) => {
                    log::warn!("sniff failed for {path:?}: {err}");
                    None
                }
            }
        };
        if !known && sniffed.is_none() {
            log::debug!("skip non-audio object {path:?}");
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(err) => {
                log::warn!("stat failed for {path:?}: {err}");
                continue;
            }
        };
        let sha256 = sha256_file(path)?;
        objects.push(AudioObject {
            id: object_id_from_path(path),
            path: path.to_path_buf(),
            extension: if known {
                ext
            } else {
                sniffed.unwrap_or_else(|| ext.clone())
            },
            detection: if known {
                DetectionKind::KnownExtension
            } else {
                DetectionKind::SniffedContent
            },
            size_bytes: metadata.len(),
            sha256,
        });
    }
    objects.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(objects)
}

/// Directory under the evidence root that holds all evidence for one object.
pub fn evidence_dir(evidence_root: &Path, object: &AudioObject) -> PathBuf {
    evidence_root.join(&object.id)
}
