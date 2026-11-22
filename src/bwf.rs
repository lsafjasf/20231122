use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::model::{
    BITS_PER_SAMPLE, BwfSnapshot, CHANNELS, DAY_SAMPLES, FPS, MarkerSpec, RecoveryEvidence,
    SAMPLE_RATE, SampleSpec, ValidationFault,
};

pub fn default_spec() -> SampleSpec {
    let time_reference_samples = u64::from(SAMPLE_RATE) * 60 * 60;
    let offsets = [48_000_u64, 96_000, 168_000, 240_000];
    let markers = offsets
        .into_iter()
        .enumerate()
        .map(|(index, sample_offset)| MarkerSpec {
            id: index as u32 + 1,
            label: format!("cue-{}", index + 1),
            sample_offset,
            absolute_timecode: samples_to_timecode(
                time_reference_samples + sample_offset,
                SAMPLE_RATE,
                FPS,
            ),
        })
        .collect();

    SampleSpec {
        duration_seconds: 6,
        sample_rate: SAMPLE_RATE,
        channels: CHANNELS,
        bits_per_sample: BITS_PER_SAMPLE,
        time_reference_samples,
        markers,
    }
}

pub fn write_fixtures(dir: &Path, spec: &SampleSpec) -> Result<(PathBuf, PathBuf)> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let full = build_wave(spec);
    let data_offset = find_data_payload_offset(&full)?;
    let data_len = u64::from(spec.duration_seconds)
        * u64::from(spec.sample_rate)
        * u64::from(spec.channels)
        * u64::from(spec.bits_per_sample / 8);
    let cut = data_offset + data_len * 7 / 10;
    if cut >= full.len() as u64 {
        bail!("truncation point is outside the generated wave");
    }

    let truncated_path = dir.join("bwf_truncated.wav");
    let recovered_path = dir.join("bwf_recovered.wav");
    fs::write(&truncated_path, &full[..cut as usize])
        .with_context(|| format!("write {}", truncated_path.display()))?;
    recover_file(&truncated_path, &recovered_path)?;
    Ok((truncated_path, recovered_path))
}

fn build_wave(spec: &SampleSpec) -> Vec<u8> {
    let total_frames = u64::from(spec.duration_seconds) * u64::from(spec.sample_rate);
    let data_len =
        total_frames as usize * usize::from(spec.channels) * usize::from(spec.bits_per_sample / 8);
    let mut data = Vec::with_capacity(data_len);

    for frame in 0..total_frames {
        let phase =
            (frame as f64 * 440.0 * std::f64::consts::TAU / f64::from(spec.sample_rate)) as f32;
        let mut value = (phase.sin() * 7_000.0) as i16;
        if spec
            .markers
            .iter()
            .any(|marker| frame >= marker.sample_offset && frame < marker.sample_offset + 240)
        {
            value = 16_000;
        }
        data.extend_from_slice(&value.to_le_bytes());
    }

    let fmt = {
        let mut bytes = Vec::with_capacity(16);
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&spec.channels.to_le_bytes());
        bytes.extend_from_slice(&spec.sample_rate.to_le_bytes());
        let block_align = spec.channels * (spec.bits_per_sample / 8);
        let byte_rate = spec.sample_rate * u32::from(block_align);
        bytes.extend_from_slice(&byte_rate.to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&spec.bits_per_sample.to_le_bytes());
        bytes
    };

    let mut chunks = Vec::new();
    append_chunk(&mut chunks, b"fmt ", &fmt);
    append_chunk(&mut chunks, b"bext", &build_bext(spec));
    append_chunk(&mut chunks, b"cue ", &build_cue(spec));
    append_chunk(&mut chunks, b"LIST", &build_labels(spec));
    append_chunk(&mut chunks, b"data", &data);

    let mut wave = Vec::with_capacity(chunks.len() + 12);
    wave.extend_from_slice(b"RIFF");
    wave.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    wave.extend_from_slice(b"WAVE");
    wave.extend_from_slice(&chunks);
    wave
}

fn build_bext(spec: &SampleSpec) -> Vec<u8> {
    let mut bytes = vec![0_u8; 602];
    put_fixed(&mut bytes[0..256], b"BWF replay lab deterministic fixture");
    put_fixed(&mut bytes[256..288], b"BWF Replay Lab");
    put_fixed(&mut bytes[288..320], b"BWF-LAB-0001");
    put_fixed(&mut bytes[320..330], b"2026-11-22");
    put_fixed(&mut bytes[330..338], b"01:00:00");
    bytes[338..346].copy_from_slice(&spec.time_reference_samples.to_le_bytes());
    bytes[346..348].copy_from_slice(&1_u16.to_le_bytes());
    let coding_history = b"A=PCM,F=48000,W=16,M=mono\r\n";
    bytes.extend_from_slice(coding_history);
    bytes
}

fn build_cue(spec: &SampleSpec) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(4 + spec.markers.len() * 24);
    bytes.extend_from_slice(&(spec.markers.len() as u32).to_le_bytes());
    for marker in &spec.markers {
        bytes.extend_from_slice(&marker.id.to_le_bytes());
        bytes.extend_from_slice(&(marker.sample_offset as u32).to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&(marker.sample_offset as u32).to_le_bytes());
    }
    bytes
}

fn build_labels(spec: &SampleSpec) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"adtl");
    for marker in &spec.markers {
        let mut payload = Vec::new();
        payload.extend_from_slice(&marker.id.to_le_bytes());
        payload.extend_from_slice(marker.label.as_bytes());
        payload.push(0);
        append_chunk(&mut bytes, b"labl", &payload);
    }
    bytes
}

fn put_fixed(dst: &mut [u8], value: &[u8]) {
    let take = dst.len().min(value.len());
    dst[..take].copy_from_slice(&value[..take]);
}

fn append_chunk(out: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
}

fn find_data_payload_offset(bytes: &[u8]) -> Result<u64> {
    let mut pos = 12_usize;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into()?) as usize;
        if id == b"data" {
            return Ok((pos + 8) as u64);
        }
        pos += 8 + size + (size & 1);
    }
    bail!("generated wave has no data chunk")
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex::encode(digest)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

pub fn samples_to_timecode(total_samples: u64, sample_rate: u32, fps: u32) -> String {
    let total_frames = total_samples.saturating_mul(u64::from(fps)) / u64::from(sample_rate);
    let frames_per_day = 24 * 60 * 60 * u64::from(fps);
    let frames = total_frames % frames_per_day;
    let ff = frames % u64::from(fps);
    let seconds = frames / u64::from(fps);
    let ss = seconds % 60;
    let minutes = seconds / 60;
    let mm = minutes % 60;
    let hh = minutes / 60;
    format!("{hh:02}:{mm:02}:{ss:02}:{ff:02}")
}

pub fn recover_file(source: &Path, destination: &Path) -> Result<RecoveryEvidence> {
    let source_bytes = fs::read(source).with_context(|| format!("read {}", source.display()))?;
    let snapshot = parse_wave(source)?;
    let Some(fault) = snapshot.structural_fault else {
        bail!("source is not truncated; recovery refused");
    };
    if fault.code != "data_truncated" {
        bail!("source fault is not a truncated data chunk: {}", fault.code);
    }

    let final_len = snapshot.data_payload_offset + snapshot.data_declared_len;
    let final_len = final_len + (final_len & 1);
    let mut recovered = source_bytes.clone();
    let appended = final_len.saturating_sub(recovered.len() as u64);
    recovered.resize(final_len as usize, 0);
    if recovered.len() >= 8 {
        let riff_size = (recovered.len() - 8) as u32;
        recovered[4..8].copy_from_slice(&riff_size.to_le_bytes());
    }
    fs::write(destination, &recovered)
        .with_context(|| format!("write {}", destination.display()))?;

    Ok(RecoveryEvidence {
        source: source.to_path_buf(),
        recovered: destination.to_path_buf(),
        truncated_at: source_bytes.len() as u64,
        appended_zero_bytes: appended,
        mode: "preserve surviving PCM, zero-pad to the BWF data length".to_owned(),
        source_sha256: sha256_hex(&source_bytes),
        recovered_sha256: sha256_hex(&recovered),
    })
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        bytes.get(offset..offset + 8)?.try_into().ok()?,
    ))
}

fn fixed_text(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).trim().to_owned()
}

pub fn parse_wave(path: &Path) -> Result<BwfSnapshot> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("{} is not a RIFF/WAVE file", path.display());
    }

    let riff_size = read_u32(&bytes, 4).unwrap_or(0);
    let mut fault: Option<ValidationFault> = None;
    let mut sample_rate = 0_u32;
    let mut channels = 0_u16;
    let mut bits_per_sample = 0_u16;
    let mut time_reference_samples = 0_u64;
    let mut data_payload_offset = 0_u64;
    let mut data_declared_len = 0_u64;
    let mut data_actual_len = 0_u64;
    let mut cue_offsets: Vec<(u32, u64)> = Vec::new();
    let mut labels: Vec<(u32, String)> = Vec::new();
    let mut pos = 12_usize;

    while pos + 8 <= bytes.len() {
        let id: [u8; 4] = bytes[pos..pos + 4].try_into().expect("four-byte chunk id");
        let declared = read_u32(&bytes, pos + 4).unwrap_or(0) as u64;
        let payload_start = pos + 8;
        let available = bytes.len().saturating_sub(payload_start) as u64;
        let actual = declared.min(available);
        let payload = &bytes[payload_start..payload_start + actual as usize];

        match &id {
            b"fmt " if payload.len() >= 16 => {
                channels = read_u16(payload, 2).unwrap_or(0);
                sample_rate = read_u32(payload, 4).unwrap_or(0);
                bits_per_sample = read_u16(payload, 14).unwrap_or(0);
            }
            b"bext" if payload.len() >= 602 => {
                time_reference_samples = read_u64(payload, 338).unwrap_or(0);
            }
            b"cue " => {
                let count = read_u32(payload, 0).unwrap_or(0) as usize;
                for index in 0..count {
                    let base = 4 + index * 24;
                    if base + 24 > payload.len() {
                        break;
                    }
                    let cue_id = read_u32(payload, base).unwrap_or(0);
                    let sample_offset = read_u32(payload, base + 20).unwrap_or(0) as u64;
                    cue_offsets.push((cue_id, sample_offset));
                }
            }
            b"LIST" if payload.len() >= 4 && &payload[0..4] == b"adtl" => {
                parse_labels(&payload[4..], &mut labels);
            }
            b"data" => {
                data_payload_offset = payload_start as u64;
                data_declared_len = declared;
                data_actual_len = actual;
                if available < declared && fault.is_none() {
                    fault = Some(ValidationFault {
                        code: "data_truncated".to_owned(),
                        message: "data chunk ends before its declared BWF payload length"
                            .to_owned(),
                        byte_offset: bytes.len() as u64,
                        expected: format!("{declared} data bytes"),
                        actual: format!("{available} data bytes"),
                    });
                }
            }
            _ => {}
        }

        if actual < declared {
            if fault.is_none() && &id != b"data" {
                fault = Some(ValidationFault {
                    code: "chunk_truncated".to_owned(),
                    message: format!(
                        "chunk {} ends before its declared length",
                        String::from_utf8_lossy(&id)
                    ),
                    byte_offset: bytes.len() as u64,
                    expected: format!("{declared} bytes"),
                    actual: format!("{available} bytes"),
                });
            }
            break;
        }
        pos = payload_start + declared as usize + (declared as usize & 1);
    }

    if data_declared_len == 0 {
        bail!("{} has no data chunk", path.display());
    }
    if sample_rate == 0 || channels == 0 || bits_per_sample == 0 {
        bail!("{} has no usable fmt chunk", path.display());
    }
    if fault.is_none() && u64::from(riff_size) + 8 > bytes.len() as u64 {
        fault = Some(ValidationFault {
            code: "riff_truncated".to_owned(),
            message: "RIFF length exceeds the bytes physically present".to_owned(),
            byte_offset: bytes.len() as u64,
            expected: format!("{} file bytes", u64::from(riff_size) + 8),
            actual: format!("{} file bytes", bytes.len()),
        });
    }

    let markers = cue_offsets
        .into_iter()
        .map(|(id, sample_offset)| {
            let label = labels
                .iter()
                .find(|(label_id, _)| *label_id == id)
                .map(|(_, label)| label.clone())
                .unwrap_or_else(|| format!("cue-{id}"));
            MarkerSpec {
                id,
                label,
                sample_offset,
                absolute_timecode: samples_to_timecode(
                    time_reference_samples + sample_offset,
                    sample_rate,
                    FPS,
                ),
            }
        })
        .collect();

    Ok(BwfSnapshot {
        path: path.to_path_buf(),
        file_len: bytes.len() as u64,
        riff_declared_len: u64::from(riff_size) + 8,
        data_payload_offset,
        data_declared_len,
        data_actual_len,
        sample_rate,
        channels,
        bits_per_sample,
        time_reference_samples,
        time_reference_timecode: samples_to_timecode(
            time_reference_samples % DAY_SAMPLES,
            sample_rate,
            FPS,
        ),
        sha256: sha256_hex(&bytes),
        markers,
        structural_fault: fault,
    })
}

fn parse_labels(mut payload: &[u8], labels: &mut Vec<(u32, String)>) {
    while payload.len() >= 8 {
        let id = &payload[0..4];
        let size = read_u32(payload, 4).unwrap_or(0) as usize;
        if payload.len() < 8 + size {
            break;
        }
        if id == b"labl" && size >= 4 {
            let label_id = read_u32(&payload[8..], 0).unwrap_or(0);
            let text = fixed_text(&payload[12..8 + size]);
            labels.push((label_id, text));
        }
        let advance = 8 + size + (size & 1);
        if advance == 0 || advance > payload.len() {
            break;
        }
        payload = &payload[advance..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn timecode_uses_bwf_reference_and_25fps() {
        let spec = default_spec();
        assert_eq!(spec.markers[0].absolute_timecode, "01:00:01:00");
        assert_eq!(spec.markers[2].absolute_timecode, "01:00:03:12");
    }

    #[test]
    fn truncated_fixture_recovers_to_declared_length() {
        let temp = tempdir().expect("tempdir");
        let spec = default_spec();
        let (truncated, recovered) = write_fixtures(temp.path(), &spec).expect("fixtures");
        let broken = parse_wave(&truncated).expect("parse truncated");
        let fixed = parse_wave(&recovered).expect("parse recovered");
        assert_eq!(
            broken
                .structural_fault
                .as_ref()
                .map(|fault| fault.code.as_str()),
            Some("data_truncated")
        );
        assert!(fixed.structural_fault.is_none());
        assert_eq!(fixed.data_actual_len, fixed.data_declared_len);
        assert_eq!(fixed.markers.len(), 4);
        assert_eq!(fixed.markers[0].label, "cue-1");
    }
}
