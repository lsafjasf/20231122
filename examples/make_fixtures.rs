use std::fs;
use std::path::Path;

use tempo_pitch_debug::wav::{write_empty_wav, Pcm16Writer};

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const DURATION_US: u64 = 3_200_000;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new("fixtures/missing_segment");
    fs::create_dir_all(dir)?;
    write_audio(&dir.join("original.wav"))?;
    write_speed(&dir.join("speed_curve.csv"))?;
    write_pitch(&dir.join("pitch_trajectory.csv"))?;
    fs::write(
        dir.join("scenario.json"),
        r#"{
  "schema": "tempo-pitch-scenario/1",
  "name": "missing-pitch-segment",
  "audio": "original.wav",
  "speed_curve": "speed_curve.csv",
  "pitch_track": "pitch_trajectory.csv",
  "output_dir": "../../artifacts/missing-segment",
  "analysed_channel": 0,
  "window_us": 20000,
  "render_quantum_us": 100000,
  "pitch_gap_threshold_us": 200000,
  "missing_span": {
    "start_us": 1250000,
    "end_us": 1500000
  }
}
"#,
    )?;
    write_empty_fixture()?;
    write_limit_fixture()?;
    println!("fixtures written to {}", dir.display());
    Ok(())
}

fn write_empty_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new("fixtures/empty_input");
    fs::create_dir_all(dir)?;
    write_empty_wav(&dir.join("empty.wav"), RATE, CHANNELS)?;
    fs::write(dir.join("speed_curve.csv"), "time_us,rate\n")?;
    fs::write(dir.join("pitch_trajectory.csv"), "time_us,semitones\n")?;
    fs::write(
        dir.join("scenario.json"),
        r#"{
  "schema": "tempo-pitch-scenario/1",
  "name": "empty-input",
  "audio": "empty.wav",
  "speed_curve": "speed_curve.csv",
  "pitch_track": "pitch_trajectory.csv",
  "output_dir": "../../artifacts/empty-input",
  "analysed_channel": 0,
  "window_us": 20000,
  "render_quantum_us": 100000,
  "pitch_gap_threshold_us": 200000
}
"#,
    )?;
    Ok(())
}

fn write_limit_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let dir = Path::new("fixtures/limit_rejected");
    fs::create_dir_all(dir)?;
    fs::write(
        dir.join("scenario.json"),
        r#"{
  "schema": "tempo-pitch-scenario/1",
  "name": "curve-limit-rejected",
  "audio": "../missing_segment/original.wav",
  "speed_curve": "../missing_segment/speed_curve.csv",
  "pitch_track": "../missing_segment/pitch_trajectory.csv",
  "output_dir": "../../artifacts/limit-rejected",
  "analysed_channel": 0,
  "window_us": 20000,
  "render_quantum_us": 100000,
  "pitch_gap_threshold_us": 200000,
  "limits": {
    "max_audio_bytes": 536870912,
    "max_curve_bytes": 67108864,
    "max_curve_points": 10,
    "max_timeline_entries": 100000,
    "max_render_segments": 20000,
    "max_decoded_pcm_bytes": 4294967296
  }
}
"#,
    )?;
    Ok(())
}

fn write_audio(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer = Pcm16Writer::create(path, RATE, CHANNELS)?;
    let total_frames = (DURATION_US as u128 * RATE as u128 / 1_000_000) as u64;
    let mut frame = 0_u64;
    while frame < total_frames {
        let count = (total_frames - frame).min(4_096) as usize;
        let mut samples = Vec::with_capacity(count * CHANNELS as usize);
        for offset in 0..count {
            let index = frame + offset as u64;
            let time = index as f64 / RATE as f64;
            let envelope = 0.24 * (0.70 + 0.30 * (2.0 * std::f64::consts::PI * 1.7 * time).sin());
            let vibrato = 0.006 * (2.0 * std::f64::consts::PI * 5.0 * time).sin();
            let left = envelope
                * ((2.0 * std::f64::consts::PI * 220.0 * time + vibrato).sin()
                    + 0.22 * (2.0 * std::f64::consts::PI * 440.0 * time).sin());
            let right = envelope
                * ((2.0 * std::f64::consts::PI * 330.0 * time + vibrato).sin()
                    + 0.18 * (2.0 * std::f64::consts::PI * 660.0 * time).sin());
            samples.push(left as f32);
            samples.push(right as f32);
        }
        writer.write_f32_interleaved(&samples)?;
        frame += count as u64;
    }
    writer.finish(path)?;
    Ok(())
}

fn write_speed(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::write(
        path,
        "time_us,rate\n\
0,1.00\n\
700000,1.10\n\
1250000,1.45\n\
1800000,0.80\n\
2400000,1.30\n\
3200000,1.00\n",
    )?;
    Ok(())
}

fn write_pitch(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut text = String::from("time_us,semitones\n");
    let mut time_us = 0_u64;
    while time_us <= DURATION_US {
        let inside_missing = time_us > 1_250_000 && time_us < 1_500_000;
        if !inside_missing {
            let time = time_us as f64 / 1_000_000.0;
            let semitones = 0.35 * (2.0 * std::f64::consts::PI * 0.45 * time).sin();
            text.push_str(&format!("{time_us},{semitones:.6}\n"));
        }
        time_us += 20_000;
    }
    fs::write(path, text)?;
    Ok(())
}
