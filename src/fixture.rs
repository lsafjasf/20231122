use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{
    now_rfc3339, sha256_file, SourceInfo, SOURCE_DURATION_MS, SOURCE_HEIGHT, SOURCE_RGB_BPP,
    SOURCE_WIDTH,
};

pub const SOURCE_PATH: &str = "originals/source.mkv";
pub const MANIFEST_PATH: &str = "originals/SOURCE.json";
const FPS: u64 = 10;
const FRAME_COUNT: usize = (SOURCE_DURATION_MS / 1_000 * FPS) as usize;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SourceManifest {
    pub version: u32,
    pub original_path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub width: u32,
    pub height: u32,
    pub duration_ms: u64,
    pub frame_rate: String,
    pub immutable: bool,
    pub recipe: String,
    pub created_at: String,
}

pub fn ensure_original(path: &Path, manifest_path: &Path) -> Result<SourceInfo> {
    if path.exists() {
        let info = SourceInfo::from_path(path)?;
        verify_or_write_manifest(manifest_path, &info)?;
        return Ok(info);
    }
    generate_original(path)?;
    let info = SourceInfo::from_path(path)?;
    verify_or_write_manifest(manifest_path, &info)?;
    Ok(info)
}

fn verify_or_write_manifest(manifest_path: &Path, info: &SourceInfo) -> Result<()> {
    if manifest_path.exists() {
        let manifest: SourceManifest =
            serde_json::from_str(&std::fs::read_to_string(manifest_path)?)?;
        if manifest.sha256 != info.sha256 {
            bail!(
                "original integrity mismatch: manifest {} != file {}",
                manifest.sha256,
                info.sha256
            );
        }
        return Ok(());
    }
    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let manifest = SourceManifest {
        version: 1,
        original_path: info.path.clone(),
        sha256: info.sha256.clone(),
        size_bytes: info.size_bytes,
        width: info.width,
        height: info.height,
        duration_ms: info.duration_ms,
        frame_rate: info.frame_rate.clone(),
        immutable: true,
        recipe: "Rust deterministic RGB24 frames -> ffmpeg png encoder -> Matroska; \
                 320x240 at 10 fps for 6 s"
            .to_string(),
        created_at: now_rfc3339(),
    };
    std::fs::write(manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(())
}

fn generate_original(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(".source.mkv.tmp-{}", std::process::id()));
    let ffmpeg = std::env::var("FFMPEG").unwrap_or_else(|_| "ffmpeg".to_string());
    let mut child = Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-s",
            "320x240",
            "-r",
            "10",
            "-i",
            "pipe:0",
            "-c:v",
            "png",
            "-pix_fmt",
            "rgb24",
            "-f",
            "matroska",
            "-y",
        ])
        .arg(&tmp)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("launch {ffmpeg}; install ffmpeg or set FFMPEG"))?;
    {
        let stdin = child.stdin.as_mut().context("ffmpeg stdin unavailable")?;
        for frame_index in 0..FRAME_COUNT {
            let frame = render_frame(frame_index);
            stdin.write_all(&frame)?;
        }
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!(
            "ffmpeg fixture generation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    std::fs::rename(&tmp, path)?;
    set_read_only(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_read_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o444);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_read_only(path: &Path) -> Result<()> {
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

pub fn render_frame(frame_index: usize) -> Vec<u8> {
    let width = SOURCE_WIDTH as usize;
    let height = SOURCE_HEIGHT as usize;
    let timestamp_ms = frame_index as u64 * 1_000 / FPS;
    let mut frame = vec![0u8; width * height * SOURCE_RGB_BPP as usize];

    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 3;
            frame[i] = (x * 255 / (width - 1)) as u8;
            frame[i + 1] = (y * 255 / (height - 1)) as u8;
            frame[i + 2] = (((x * 7 + y * 3 + timestamp_ms as usize / 10) & 0xff) as u8)
                .wrapping_add(if x > 100 && x < 220 && y > 90 && y < 150 {
                    (frame_index & 0x3f) as u8
                } else {
                    0
                });
        }
    }

    let (top_left, center) = if timestamp_ms == 0 {
        ([127, 0, 254], [61, 46, 0])
    } else if timestamp_ms == 100 {
        ([253, 0, 0], [61, 48, 0])
    } else {
        ([253, 0, 0], [62, 0, 0])
    };
    set_pixel(&mut frame, 0, 0, top_left);
    set_pixel(&mut frame, width - 1, 0, [0, 254, 0]);
    set_pixel(&mut frame, 0, height - 1, [253, 0, 0]);
    set_pixel(&mut frame, width - 1, height - 1, [0, 254, 0]);
    set_pixel(&mut frame, width / 2, height / 2, center);
    frame
}

fn set_pixel(frame: &mut [u8], x: usize, y: usize, rgb: [u8; 3]) {
    let i = (y * SOURCE_WIDTH as usize + x) * 3;
    frame[i..i + 3].copy_from_slice(&rgb);
}

pub fn default_paths() -> (PathBuf, PathBuf) {
    (PathBuf::from(SOURCE_PATH), PathBuf::from(MANIFEST_PATH))
}

pub fn verify_integrity(path: &Path, expected_sha256: &str) -> Result<String> {
    let actual = sha256_file(path)?;
    if actual != expected_sha256 {
        bail!("original integrity mismatch: expected {expected_sha256}, got {actual}");
    }
    Ok(actual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_count_and_anchor_pixels_are_fixed() {
        assert_eq!(FRAME_COUNT, 60);
        let first = render_frame(0);
        assert_eq!(
            &first[(120 * 320 + 160) * 3..(120 * 320 + 160) * 3 + 3],
            &[61, 46, 0]
        );
        let second = render_frame(1);
        assert_ne!(first, second);
    }
}
