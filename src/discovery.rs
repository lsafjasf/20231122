use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::failure::{Failure, Result};
use crate::sha256;

#[derive(Debug, Clone, Serialize)]
pub struct AudioProbe {
    pub path: PathBuf,
    pub file_name: String,
    pub extension: Option<String>,
    pub extension_known: bool,
    pub size_bytes: u64,
    pub sha256: String,
    pub magic_hex: String,
    pub container_hint: String,
    pub gstreamer_caps: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedManifest {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub candidates: Vec<PathBuf>,
}

const KNOWN_AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "wave", "flac", "ogg", "oga", "opus", "mp3", "m4a", "aac", "aif", "aiff", "caf", "wma",
    "webm", "mka", "au", "snd", "amr", "ape",
];

pub fn find_manifest_candidates(audio: &Path) -> Vec<PathBuf> {
    let parent = audio.parent().unwrap_or_else(|| Path::new("."));
    let stem = audio
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("replay");
    let names = [
        format!("{stem}.json"),
        format!("{stem}.manifest.json"),
        format!("{stem}.assessment.json"),
        "replay.json".to_string(),
        "manifest.json".to_string(),
    ];
    let mut candidates = Vec::new();
    for name in names {
        let path = parent.join(name);
        if path.is_file() && !candidates.contains(&path) {
            candidates.push(path);
        }
    }
    candidates
}

pub fn resolve_manifest(
    explicit_manifest: Option<&Path>,
    audio: Option<&Path>,
) -> Result<Option<ResolvedManifest>> {
    let (path, candidates) = if let Some(path) = explicit_manifest {
        if !path.is_file() {
            return Err(Failure::missing_file(
                "评估清单",
                path,
                "传入 `--manifest <清单.json>`，或直接传入音频并让程序查找同名清单。",
            ));
        }
        (path.to_path_buf(), vec![path.to_path_buf()])
    } else if let Some(audio) = audio {
        let candidates = find_manifest_candidates(audio);
        let Some(path) = candidates.first().cloned() else {
            return Ok(None);
        };
        (path, candidates)
    } else {
        return Ok(None);
    };

    let bytes = fs::read(&path).map_err(|e| Failure::io("读取评估清单", &path, e))?;
    Ok(Some(ResolvedManifest {
        path,
        bytes,
        candidates,
    }))
}

pub fn probe_audio(path: &Path) -> Result<AudioProbe> {
    let metadata = fs::metadata(path).map_err(|e| Failure::io("读取音频元数据", path, e))?;
    if !metadata.is_file() {
        return Err(Failure::new(
            "audio_not_file",
            format!("音频路径不是普通文件：{}", path.display()),
            "选择实际的重放文件，而不是目录或设备节点。",
        ));
    }

    let prefix = read_prefix(path, 64)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase());
    let extension_known = extension
        .as_deref()
        .is_some_and(|value| KNOWN_AUDIO_EXTENSIONS.contains(&value));
    let (container_hint, _mime) = sniff_container(&prefix);

    Ok(AudioProbe {
        path: path.to_path_buf(),
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("audio")
            .to_string(),
        extension,
        extension_known,
        size_bytes: metadata.len(),
        sha256: sha256::file_sha256(path)?,
        magic_hex: prefix
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" "),
        container_hint: container_hint.to_string(),
        gstreamer_caps: None,
    })
}

fn read_prefix(path: &Path, limit: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut file = fs::File::open(path).map_err(|e| Failure::io("打开音频", path, e))?;
    let mut buffer = vec![0_u8; limit];
    let mut filled = 0;
    while filled < limit {
        let read = file
            .read(&mut buffer[filled..])
            .map_err(|e| Failure::io("读取音频文件头", path, e))?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

pub fn sniff_container(prefix: &[u8]) -> (&'static str, &'static str) {
    if prefix.starts_with(b"RIFF") && prefix.get(8..12) == Some(b"WAVE") {
        ("RIFF/WAVE", "audio/x-wav")
    } else if prefix.starts_with(b"RF64") && prefix.get(8..12) == Some(b"WAVE") {
        ("RF64/WAVE", "audio/x-wav")
    } else if prefix.starts_with(b"FORM")
        && matches!(prefix.get(8..12), Some(b"AIFF") | Some(b"AIFC"))
    {
        ("AIFF", "audio/x-aiff")
    } else if prefix.starts_with(b"OggS") {
        ("Ogg", "application/ogg")
    } else if prefix.starts_with(b"fLaC") {
        ("FLAC", "audio/flac")
    } else if prefix.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        ("EBML/Matroska", "video/x-matroska")
    } else if prefix.get(4..8) == Some(b"ftyp") {
        ("ISO BMFF", "audio/mp4")
    } else if prefix.starts_with(b"ID3") || prefix.starts_with(&[0xff, 0xfb]) {
        ("MPEG audio", "audio/mpeg")
    } else if prefix.starts_with(b"#!AMR") {
        ("AMR", "audio/amr")
    } else if prefix.starts_with(b".snd") {
        ("Sun AU", "audio/x-au")
    } else if prefix.is_empty() {
        ("empty", "application/octet-stream")
    } else {
        ("unrecognized", "application/octet-stream")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wave_and_unknown_content() {
        let mut wav = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        wav.resize(32, 0);
        assert_eq!(sniff_container(&wav).0, "RIFF/WAVE");
        assert_eq!(sniff_container(b"not an audio file").0, "unrecognized");
    }
}
