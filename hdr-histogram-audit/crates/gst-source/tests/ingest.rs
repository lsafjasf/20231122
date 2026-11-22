use gst_source::{collect_samples, init, pipeline_description, SourceConfig};

#[test]
fn pipeline_description_contains_audited_element_chain() {
    let config = SourceConfig::default();
    let description = pipeline_description(&config);
    assert!(description.contains("videotestsrc"));
    assert!(description.contains("videoconvert"));
    assert!(description.contains("video/x-raw,format=RGB"));
    assert!(description.contains("appsink name=audit_appsink"));
}

#[test]
fn collects_one_traced_sample_per_buffer() {
    init().expect("gstreamer initializes");
    let config = SourceConfig {
        frame_count: 4,
        mastering_peak_nits: 1000.0,
        sink_name: "audit_appsink",
    };
    let samples = collect_samples(&config).expect("pipeline runs to completion");

    assert_eq!(samples.len(), 4);
    for (index, sample) in samples.iter().enumerate() {
        assert_eq!(sample.sample_id, format!("gst-{index:04}"));
        assert_eq!(sample.source_element, "audit_appsink");
        assert!(sample.pts_ns.is_some(), "PTS is retained as source evidence");
        assert!(sample.nits.is_finite() && sample.nits >= 0.0);
    }

    // Feeding the captured GStreamer samples through the audit engine must
    // work end to end and keep every sample id for provenance.
    let audit_config = audit_core::PipelineConfig {
        headroom_nits: config.mastering_peak_nits,
        ..Default::default()
    };
    let run = audit_core::run(&samples, &audit_config);
    assert_eq!(run.records.len(), samples.len());
    for record in &run.records {
        assert!(record
            .steps
            .iter()
            .all(|step| step.evidence.is_some() || step.input_value.is_finite()));
    }
}
