use std::f32::consts::TAU;
use std::fs;
use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new("examples");
    fs::create_dir_all(root.join("new"))?;
    fs::create_dir_all(root.join("legacy"))?;

    write_recovered_wav(root.join("new/recovered_replay.rplx"), 48_000, 3.2)?;
    fs::write(root.join("new/manifest.json"), NEW_MANIFEST)?;

    write_recovered_wav(root.join("legacy/legacy_replay.wav"), 8_000, 1.2)?;
    fs::write(root.join("legacy/manifest.json"), LEGACY_MANIFEST)?;
    println!("fixtures written under {}", root.display());
    Ok(())
}

fn write_recovered_wav(
    path: impl AsRef<Path>,
    sample_rate: u32,
    seconds: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    let total = (sample_rate as f32 * seconds) as u32;
    for index in 0..total {
        let time = index as f32 / sample_rate as f32;
        let carrier = (TAU * 220.0 * time).sin() * 0.22 + (TAU * 440.0 * time).sin() * 0.08;
        let syllable = 0.68 + 0.24 * (TAU * 1.7 * time).sin();
        let mut sample = carrier * syllable;
        if (1.850..1.970).contains(&time) {
            let phase = ((time - 1.850) / 0.120).clamp(0.0, 1.0);
            sample *= 0.08 + 0.92 * (1.0 - phase);
            sample += (TAU * 510.0 * time).sin() * 0.012 * (1.0 - phase);
        }
        if (2.450..2.510).contains(&time) {
            sample *= 0.55;
        }
        writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    writer.finalize()?;
    Ok(())
}

const NEW_MANIFEST: &str = r#"{
  "format": "audio-plc-assessment",
  "format_version": 2,
  "case_id": "recovery-replay-2026-11-22",
  "recovered_at": "2026-11-22T03:20:00Z",
  "replay_origin": "recovery replay start (0:00)",
  "source": {
    "name": "Ticket ALC-1042",
    "note": "Recovered replay kept in its original unknown extension"
  },
  "audio": {
    "path": "recovered_replay.rplx",
    "container_note": "content-sniffed by GStreamer"
  },
  "tracks": [
    {
      "id": "caller",
      "index": 0,
      "name": "Recovered caller",
      "codec": "pcm_s16le",
      "sample_rate": 48000,
      "channels": 1,
      "frame_size": 960
    }
  ],
  "processing": [
    {
      "id": "plc-wsola",
      "name": "waveform-similarity",
      "parameters": {
        "algorithm": "wsola",
        "fade_ms": 12,
        "attenuation_db": -3,
        "concealment_frames": 6
      }
    }
  ],
  "events": [
    {
      "id": "loss-001",
      "kind": "packet_loss",
      "timestamp_ms": 1850,
      "duration_ms": 120,
      "track_id": "caller",
      "frame": 92,
      "processing_id": "plc-wsola",
      "parameters": {
        "search_window_ms": 20
      }
    },
    {
      "id": "loss-002",
      "kind": "packet_loss",
      "timestamp_ms": 2450,
      "duration_ms": 60,
      "track_id": "caller",
      "frame": 122,
      "processing_id": "plc-wsola"
    }
  ]
}
"#;

const LEGACY_MANIFEST: &str = r#"{
  "version": 1,
  "case_id": "legacy-2023-11-22",
  "audio_path": "legacy_replay.wav",
  "sample_rate": 8000,
  "channels": 1,
  "frame_size": 160,
  "plc_params": {
    "algorithm": "repeat-last-frame",
    "fade_ms": 8
  },
  "events": [
    {
      "id": "legacy-loss-1",
      "t": 0.45,
      "duration": 0.08,
      "track": 0,
      "frame_index": 22,
      "plc_params": {
        "repeat_count": 4
      }
    }
  ]
}
"#;
