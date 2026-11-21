//! End-to-end tests for the PLC evaluation core.

use plc_eval::eval;
use plc_eval::loss::{LossKind, LossManifest};
use plc_eval::signal::Synth;
use plc_eval::wav::{Wav, WavSpec};
use plc_eval::{Strategy, FRAME_MS};

const RATE: u32 = 16_000;

fn stereo_clean(duration: f32) -> Wav {
    let mut sl = Synth::new(0x5EED_1234);
    let mut sr = Synth::new(0x5EED_9876);
    let left = sl.voice(duration, RATE, 120.0);
    let right = sr.voice(duration, RATE, 210.0);
    let mut samples = Vec::with_capacity(left.len() * 2);
    for i in 0..left.len() {
        samples.push(left[i]);
        samples.push(right[i]);
    }
    Wav {
        spec: WavSpec {
            sample_rate: RATE,
            channels: 2,
        },
        samples,
    }
}

fn voiced_reports(kind: LossKind) -> Vec<eval::GapReport> {
    let clean = stereo_clean(5.9);
    let total_frames = (clean.frames() * 1000) / (RATE as usize * FRAME_MS);
    let channel = if kind == LossKind::OneSided { 1 } else { 0 };
    let manifest = LossManifest::generate(kind, total_frames, channel);
    let degraded = manifest.degrade(&clean);
    eval::evaluate(&clean, &degraded, &manifest)
}

#[test]
fn wav_roundtrip() {
    let clean = stereo_clean(0.5);
    let path = std::env::temp_dir().join("plc_eval_test_roundtrip.wav");
    clean.write(&path).unwrap();
    let back = Wav::read(&path).unwrap();
    assert_eq!(back.spec.sample_rate, RATE);
    assert_eq!(back.spec.channels, 2);
    assert_eq!(back.samples.len(), clean.samples.len());
    let max_err = clean
        .samples
        .iter()
        .zip(&back.samples)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(max_err < 1e-3, "max err {max_err}");
    std::fs::remove_file(&path).ok();
}

#[test]
fn manifest_roundtrip() {
    let m = LossManifest::generate(LossKind::Burst, 200, 0);
    let parsed = LossManifest::from_text(&m.to_text()).unwrap();
    assert_eq!(parsed.gaps.len(), m.gaps.len());
    assert_eq!(parsed.kind, m.kind);
    assert_eq!(parsed.gaps[3].start_frame, m.gaps[3].start_frame);
}

#[test]
fn isolated_loss_prefers_interpolation() {
    let reports = voiced_reports(LossKind::Isolated);
    let voiced: Vec<_> = reports.iter().filter(|r| r.ref_rms > 0.02).collect();
    assert!(!voiced.is_empty());
    let wins = voiced
        .iter()
        .filter(|r| r.best == Strategy::Interpolate)
        .count();
    assert!(
        wins as f32 / voiced.len() as f32 > 0.5,
        "interpolate won {wins}/{}",
        voiced.len()
    );
}

#[test]
fn trailing_burst_prefers_repeat() {
    let reports = voiced_reports(LossKind::Burst);
    let last = reports.last().expect("burst has gaps");
    assert_eq!(last.best, Strategy::RepeatLastFrame);
}

#[test]
fn one_sided_prefers_cross_channel_interpolation() {
    let reports = voiced_reports(LossKind::OneSided);
    let voiced: Vec<_> = reports.iter().filter(|r| r.ref_rms > 0.02).collect();
    let wins = voiced
        .iter()
        .filter(|r| r.best == Strategy::Interpolate)
        .count();
    assert_eq!(wins, voiced.len(), "cross-channel copy should always win");
}

#[test]
fn best_strategy_beats_silence_energy_error() {
    for kind in LossKind::ALL {
        let reports = voiced_reports(kind);
        let voiced: Vec<_> = reports.iter().filter(|r| r.ref_rms > 0.02).collect();
        let mean_red =
            voiced.iter().map(|r| r.reduction_db).sum::<f32>() / voiced.len().max(1) as f32;
        assert!(
            mean_red > 3.0,
            "{kind}: reduction {mean_red:.2} dB too small"
        );
    }
}

#[test]
fn recovered_file_has_no_zero_gaps() {
    let clean = stereo_clean(5.9);
    let total_frames = (clean.frames() * 1000) / (RATE as usize * FRAME_MS);
    let manifest = LossManifest::generate(LossKind::Isolated, total_frames, 0);
    let degraded = manifest.degrade(&clean);
    let reports = eval::evaluate(&clean, &degraded, &manifest);
    let recovered = eval::recover(&degraded, &manifest, &reports);
    // every gap region must carry non-trivial energy after recovery
    let frame_len = RATE as usize * FRAME_MS / 1000;
    for (gap, report) in manifest.gaps.iter().zip(&reports) {
        if report.ref_rms <= 0.02 {
            continue; // original was a pause
        }
        let start = gap.start_frame * frame_len;
        let end = ((gap.start_frame + gap.frames) * frame_len).min(recovered.frames());
        let mut e = 0.0f32;
        for f in start..end {
            let s = recovered.samples[f * 2];
            e += s * s;
        }
        assert!(
            e / (end - start) as f32 > 1e-4,
            "gap {} still silent",
            gap.start_frame
        );
    }
}
