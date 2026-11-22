#![cfg(feature = "gst")]
//! GStreamer 1.28 media-pipeline evidence tests.
//!
//! These exercise real elements (appsrc, vp8enc, webmmux, decodebin,
//! videorate, fakesink) rather than mocks, proving the VFR synthesis and
//! rate-normalization behavior end to end.

use vfr_replay_lab::media::{build_vfr_fixture, probe_pts, run_preview, VFR_DELTAS_NS};

fn expected_pts() -> Vec<u64> {
    let mut pts = vec![0u64];
    let mut t = 0;
    for d in VFR_DELTAS_NS {
        t += d;
        pts.push(t);
    }
    pts
}

#[test]
fn synthesized_fixture_is_genuinely_vfr_and_round_trips() {
    let dir = std::env::temp_dir().join("vfr_lab_media_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("vfr.webm");

    let written_pts = build_vfr_fixture(path.to_str().unwrap(), 160, 120).unwrap();
    assert_eq!(written_pts, expected_pts());

    // The decoded PTS must match what was synthesized — VFR timing survives
    // encode/mux/decode rather than being collapsed to a constant rate.
    let decoded = probe_pts(&format!("file://{}", path.display())).unwrap();
    assert_eq!(decoded, written_pts);

    let deltas: Vec<u64> = decoded.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        deltas.windows(2).any(|w| w[0] != w[1]),
        "expected VFR deltas"
    );
}

#[test]
fn videorate_normalizes_vfr_to_constant_output() {
    let dir = std::env::temp_dir().join("vfr_lab_media_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("vfr2.webm");
    build_vfr_fixture(path.to_str().unwrap(), 160, 120).unwrap();
    let uri = format!("file://{}", path.display());

    for (num, step_ns) in [(24, 41_666_667u64), (60, 16_666_667u64)] {
        let result = run_preview(&uri, num, 1).unwrap();
        assert!(result.success, "preview failed: {:?}", result.error);
        assert!(result.frames_out >= result.frames_in);
        // Output PTS spacing must be the constant target frame period
        // (GStreamer accumulates integer-ns PTS, so a 1ns rounding drift in
        // the running timestamp is accepted).
        let out_deltas: Vec<u64> = result
            .output_pts_ns
            .windows(2)
            .map(|w| w[1] - w[0])
            .collect();
        for delta in out_deltas {
            assert!(
                delta.abs_diff(step_ns) <= 1,
                "output not normalized to {num} fps: {delta} vs {step_ns}"
            );
        }
    }
}

#[test]
fn missing_file_is_reported_not_panicked() {
    let result = run_preview("file:///nonexistent/ghost.webm", 24, 1).unwrap();
    assert!(!result.success);
    assert!(result.error.is_some());
}
