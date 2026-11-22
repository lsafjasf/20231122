use std::time::Duration;

use encode_panel::pipeline::{LiveCapture, PipelineConfig};

#[test]
#[ignore = "requires the GStreamer runtime and x264enc plugin"]
fn live_gstreamer_pipeline_emits_encoder_samples() {
    let config = PipelineConfig {
        width: 320,
        height: 180,
        fps_numerator: 60,
        fps_denominator: 1,
        bitrate_kbps: 500,
        gop_frames: 10,
        duration_ms: 1_000,
        drop_probability: 0.1,
        appsink_max_buffers: 1,
        track: 42,
    };
    let mut capture = LiveCapture::start(config).expect("start GStreamer");
    let mut samples = Vec::new();
    let report = capture.capture_into(Duration::from_millis(1_200), |sample| {
        samples.push(sample);
    });
    assert!(!samples.is_empty(), "no samples: {report:?}");
    assert!(samples.iter().all(|sample| sample.track == 42));
    assert!(
        samples
            .windows(2)
            .all(|pair| pair[1].timestamp_ns >= pair[0].timestamp_ns)
    );
    assert!(report.bus_errors.is_empty(), "{:?}", report.bus_errors);
}
