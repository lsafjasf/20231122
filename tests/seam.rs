use loop_joiner::dsp;
use loop_joiner::samples;

// For every shipped sample the repaired loop must remove the wrap click
// and must not make the head/tail energy balance worse.
#[test]
fn samples_loop_without_audible_seam() {
    for (name, wav) in samples::all() {
        let sr = wav.sample_rate;
        let mono = wav.mono();
        let naive = dsp::seam_metrics(&mono, sr);
        let candidates = dsp::find_candidates(&mono, sr, 120.0, 8);
        assert!(!candidates.is_empty(), "{name}: no candidates");
        let cut = candidates[0];
        let fade = (0.030 * sr as f64) as usize;
        let lp = dsp::build_loop(&mono, &cut, fade, dsp::FadeCurve::EqualPower);
        let fixed = dsp::seam_metrics(&lp, sr);
        assert!(
            fixed.discontinuity < 0.05,
            "{name}: residual discontinuity {:.4}",
            fixed.discontinuity
        );
        assert!(
            fixed.discontinuity < naive.discontinuity * 0.5,
            "{name}: {:.4} not better than naive {:.4}",
            fixed.discontinuity,
            naive.discontinuity
        );
        assert!(
            fixed.energy_diff_db.abs() < naive.energy_diff_db.abs() + 0.5,
            "{name}: energy diff worsened"
        );
    }
}

// The crossfaded loop must be continuous at the wrap by construction.
#[test]
fn crossfade_wrap_is_continuous() {
    let sr = 44100;
    let mono: Vec<f32> = (0..sr)
        .map(|i| ((i as f32 * 0.013).sin() * 0.6) + (i as f32 * 0.0007).cos() * 0.3)
        .collect();
    let c = dsp::Candidate {
        start: 1000,
        end: 43000,
        score: 0.0,
        value_jump: 0.0,
        slope_jump: 0.0,
        band_mismatch: 0.0,
    };
    for curve in dsp::FadeCurve::ALL {
        let lp = dsp::build_loop(&mono, &c, 500, curve);
        let m = dsp::seam_metrics(&lp, sr);
        assert!(
            m.discontinuity < 0.02,
            "{:?}: wrap step {:.4}",
            curve,
            m.discontinuity
        );
    }
}

// Drift accounting must accumulate linearly over continuous playback.
#[test]
fn drift_accumulates_linearly() {
    let d = dsp::compute_drift(44107, Some(44100));
    assert_eq!(d.residual_per_loop, 7);
    assert_eq!(d.cumulative(100), 700);
    let d = dsp::compute_drift(44093, Some(44100));
    assert_eq!(d.residual_per_loop, -7);
    let d = dsp::compute_drift(12345, None);
    assert_eq!(d.residual_per_loop, 0);
}
