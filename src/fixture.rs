use crate::metadata::FrameMetadata;
use crate::model::{
    CameraProfile, ExtremeProfile, METADATA_RECORD_BYTES, MasteringDisplay, Rect, RepeatedObject,
};
use anyhow::Context;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

pub const DEMO_WIDTH: u32 = 128;
pub const DEMO_HEIGHT: u32 = 72;
pub const DEMO_FRAME_COUNT: u32 = 150;
pub const DEMO_ACCEPTANCE_FRAME: u32 = 72;

pub fn demo_profile() -> CameraProfile {
    CameraProfile {
        schema_version: 1,
        camera_model: "HDR Camera Simulator / XTREME-65535".into(),
        width: DEMO_WIDTH,
        height: DEMO_HEIGHT,
        fps_num: 30,
        fps_den: 1,
        frame_count: DEMO_FRAME_COUNT,
        video_format: "GRAY16_LE".into(),
        source_video_sha256: String::new(),
        fault_profile: ExtremeProfile::camera_fault(),
        repeated_object: RepeatedObject {
            id: "calibration-patch-extreme".into(),
            rect: Rect {
                x: 88,
                y: 14,
                width: 30,
                height: 28,
            },
            peak_nits: 10_000,
            threshold_nits: 8_000,
            frames: vec![42, 72, 102, 132],
        },
        repaired_mastering: MasteringDisplay {
            max_nits: 10_000,
            min_nits: 1,
            white_point: [15_635, 16_450],
        },
    }
}

pub fn generate_demo_fixture(root: &Path) -> anyhow::Result<CameraProfile> {
    fs::create_dir_all(root)?;
    let video_path = root.join("video.gray16");
    let metadata_path = root.join("metadata.bin");
    let profile_path = root.join("camera.json");
    let video_tmp = root.join(".video.gray16.tmp");
    let metadata_tmp = root.join(".metadata.bin.tmp");

    let video_file = File::create(&video_tmp)?;
    let mut video_writer = BufWriter::new(video_file);
    let mut video_hasher = Sha256::new();
    for frame in 0..DEMO_FRAME_COUNT {
        let pixels = render_frame(frame);
        video_writer.write_all(&pixels)?;
        video_hasher.update(&pixels);
    }
    video_writer.flush()?;
    video_writer.get_ref().sync_all()?;
    drop(video_writer);
    fs::rename(&video_tmp, &video_path)?;

    let video_sha256 = hex_lower(&video_hasher.finalize());
    let mut profile = demo_profile();
    profile.source_video_sha256 = video_sha256.clone();
    profile.validate()?;

    let metadata_file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&metadata_tmp)?;
    let mut metadata_writer = BufWriter::new(metadata_file);
    for frame in 0..DEMO_FRAME_COUNT {
        let pts_ns = profile.pts_ns(frame);
        let offset = profile.video_offset(frame);
        let record = FrameMetadata::camera_fault(frame, pts_ns, offset, &profile);
        metadata_writer.write_all(&record.encode())?;
    }
    metadata_writer.flush()?;
    metadata_writer.get_ref().sync_all()?;
    drop(metadata_writer);
    fs::rename(&metadata_tmp, &metadata_path)?;

    let data = serde_json::to_vec_pretty(&profile)?;
    let tmp = root.join(".camera.json.tmp");
    fs::write(&tmp, data)?;
    File::open(&tmp)?.sync_all()?;
    fs::rename(&tmp, &profile_path)?;
    write_fixture_readme(root, &profile)?;
    Ok(profile)
}

pub fn load_profile(path: &Path) -> anyhow::Result<CameraProfile> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let profile: CameraProfile = serde_json::from_slice(&data)?;
    profile.validate()?;
    Ok(profile)
}

pub fn hash_file(path: &Path) -> anyhow::Result<String> {
    let data = fs::read(path).with_context(|| format!("hash {}", path.display()))?;
    Ok(hex_lower(&Sha256::digest(data)))
}

pub fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub fn render_frame(frame: u32) -> Vec<u8> {
    let profile = demo_profile();
    let width = DEMO_WIDTH as usize;
    let height = DEMO_HEIGHT as usize;
    let mut out = Vec::with_capacity(width * height * 2);
    let band_x = ((frame as i32 * 2) % width as i32) as usize;
    let object = profile.repeated_object.frames.contains(&frame);
    for y in 0..height {
        for x in 0..width {
            let mut nits = 1_200
                + ((x * 1_400) / width)
                + ((y * 500) / height)
                + if x.abs_diff(band_x) < 4 { 900 } else { 0 };
            if object {
                let r = &profile.repeated_object.rect;
                let in_x = x >= r.x as usize && x < (r.x + r.width) as usize;
                let in_y = y >= r.y as usize && y < (r.y + r.height) as usize;
                if in_x && in_y {
                    let dx = (x - r.x as usize) as u32;
                    let dy = (y - r.y as usize) as u32;
                    nits = 3_000
                        + dx as usize * 3_500 / (r.width.saturating_sub(2) as usize)
                        + dy as usize * 3_500 / (r.height.saturating_sub(2) as usize);
                }
            }
            let code = nits.min(10_000) * 65_535 / 10_000;
            out.extend_from_slice(&(code as u16).to_le_bytes());
        }
    }
    out
}

pub fn validate_source(fixture_root: &Path, profile: &CameraProfile) -> anyhow::Result<()> {
    let video = fixture_root.join("video.gray16");
    let metadata = fixture_root.join("metadata.bin");
    let expected_video = profile.frame_bytes() * u64::from(profile.frame_count);
    let expected_metadata = METADATA_RECORD_BYTES as u64 * u64::from(profile.frame_count);
    anyhow::ensure!(
        fs::metadata(&video)?.len() == expected_video,
        "video size mismatch"
    );
    anyhow::ensure!(
        fs::metadata(&metadata)?.len() == expected_metadata,
        "metadata size mismatch"
    );
    anyhow::ensure!(
        hash_file(&video)? == profile.source_video_sha256,
        "source video hash mismatch"
    );
    Ok(())
}

fn write_fixture_readme(root: &Path, profile: &CameraProfile) -> anyhow::Result<()> {
    let frame = DEMO_ACCEPTANCE_FRAME;
    let text = format!(
        "# Camera XTREME-65535 fixture\n\n\
         Deterministic raw camera capture: {}x{}, {} frames at {}/{} fps.\n\n\
         - Acceptance time: {:.3} s\n\
         - Frame index: {}\n\
         - Raw video byte offset: 0x{:X} ({} bytes)\n\
         - Metadata byte offset: 0x{:X} ({} bytes)\n\
         - Repeated object frames: {:?}\n\
         - Camera fault profile: {}\n\
         - Original video SHA-256: {}\n",
        profile.width,
        profile.height,
        profile.frame_count,
        profile.fps_num,
        profile.fps_den,
        profile.pts_ns(frame) as f64 / 1_000_000_000.0,
        frame,
        profile.video_offset(frame),
        profile.video_offset(frame),
        profile.metadata_offset(frame),
        profile.metadata_offset(frame),
        profile.repeated_object.frames,
        profile.fault_profile.id,
        profile.source_video_sha256,
    );
    fs::write(root.join("README.md"), text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_fixture_has_stable_acceptance_geometry() {
        let dir = tempfile::tempdir().unwrap();
        let profile = generate_demo_fixture(dir.path()).unwrap();
        assert_eq!(profile.frame_bytes(), 18_432);
        assert_eq!(profile.pts_ns(72), 2_400_000_000);
        assert_eq!(profile.video_offset(72), 1_327_104);
        assert_eq!(profile.metadata_offset(72), 4_608);
        assert_eq!(profile.video_offset(72), 0x144000);
        assert_eq!(profile.metadata_offset(72), 0x1200);
        validate_source(dir.path(), &profile).unwrap();
    }
}
