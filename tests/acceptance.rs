use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicU64, Ordering};

use mouse_trail_rebuild::analyze::{self, AnalyzeOptions};
use mouse_trail_rebuild::evidence;
use mouse_trail_rebuild::model::FrameSample;

static GST_INIT: Once = Once::new();
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn init_gst() {
    GST_INIT.call_once(|| gstreamer::init().expect("GStreamer init"));
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn temp_dir(label: &str) -> PathBuf {
    let id = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "mouse-trail-test-{}-{id}-{label}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("create temp directory");
    path
}

fn options(video: &Path, events: &Path, out: &Path) -> AnalyzeOptions {
    AnalyzeOptions {
        video: video.to_path_buf(),
        events: events.to_path_buf(),
        out: out.to_path_buf(),
        max_events: 1_000_000,
        max_event_bytes: 134_217_728,
        start_ns: 0,
        end_ns: 0,
    }
}

#[test]
fn three_anchors_have_exact_times_offsets_and_clamping() {
    init_gst();
    let root = root();
    let video = root.join("artifacts/original/screen.mkv");
    let events = root.join("artifacts/original/mouse-events.jsonl");
    let out = temp_dir("anchors");
    let result = analyze::run(options(&video, &events, &out)).expect("analyze");
    assert_eq!(result.status, "ok");
    assert_eq!(result.anchor_count, 3);

    let evidence = evidence::read_evidence(&out.join("evidence.json")).expect("evidence");
    assert!(evidence.source_unchanged);
    assert_eq!(evidence.inputs.video.frames, 40);
    assert_eq!(evidence.inputs.video.duration_ns, 4_000_000_000);
    assert_eq!(evidence.counts.duplicates_removed, 1);
    assert_eq!(evidence.counts.raw_order_inversions, 1);

    let expected = [
        ("A", 1_000_000_000, 10, 942, 128.0, 96.0, 128.0, 96.0),
        ("B", 2_000_000_000, 20, 1931, 200.0, 105.0, 200.0, 105.0),
        ("C", 3_000_000_000, 30, 2741, 365.0, -12.0, 319.0, 0.0),
    ];
    for (anchor, (id, pts, frame, offset, raw_x, raw_y, x, y)) in
        evidence.anchors.iter().zip(expected)
    {
        assert_eq!(anchor.id, id);
        assert_eq!(anchor.pts_ns, pts);
        assert_eq!(anchor.frame_index, frame);
        assert_eq!(anchor.track_id, 0);
        assert_eq!(anchor.raw_offset, offset);
        assert_eq!(
            (anchor.raw_x, anchor.raw_y),
            (raw_x, raw_y),
            "raw anchor {id}"
        );
        assert_eq!(
            (anchor.output_x, anchor.output_y),
            (x, y),
            "output anchor {id}"
        );
    }
    assert!(
        evidence.anchors[0]
            .flags
            .contains(&"duplicate_timestamp".into())
    );
    assert!(
        evidence.anchors[1]
            .flags
            .contains(&"raw_order_inversion".into())
    );
    assert!(evidence.anchors[2].flags.contains(&"clamp_x".into()));
    assert!(evidence.anchors[2].flags.contains(&"clamp_y".into()));

    let original = fs::read(&events).expect("event bytes");
    for anchor in &evidence.anchors {
        let start = anchor.raw_offset as usize;
        let end = start + anchor.raw_length as usize;
        let preserved =
            fs::read(out.join(format!("evidence/raw/anchor-{}.jsonl", anchor.id))).unwrap();
        assert_eq!(preserved, original[start..end]);
    }

    let verify = analyze::verify(&out).expect("verify");
    assert!(verify.starts_with("PASS A@1.000s frame=10 raw_offset=942"));
    fs::remove_dir_all(out).ok();
}

#[test]
fn empty_events_still_produce_a_hidden_waveform() {
    init_gst();
    let root = root();
    let out = temp_dir("empty-events");
    let empty = out.join("empty.jsonl");
    fs::write(&empty, b"").unwrap();
    let result = analyze::run(options(
        &root.join("artifacts/original/screen.mkv"),
        &empty,
        &out.join("result"),
    ))
    .expect("empty events are valid");
    assert_eq!(result.status, "ok_empty_events");
    let evidence = evidence::read_evidence(&out.join("result/evidence.json")).unwrap();
    assert!(evidence.anchors.is_empty());
    assert!(evidence.error.is_none());
    let trace = fs::read_to_string(out.join("result/trace.jsonl")).unwrap();
    let samples: Vec<FrameSample> = trace
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(samples.len(), 40);
    assert!(samples.iter().all(|sample| !sample.visible));
    assert!(
        samples
            .iter()
            .all(|sample| sample.flags == vec!["stale_hidden".to_string()])
    );
    fs::remove_dir_all(out).ok();
}

#[test]
fn oversized_event_bytes_are_rejected_before_parsing() {
    init_gst();
    let out = temp_dir("oversized");
    let oversized = out.join("oversized.jsonl");
    File::create(&oversized)
        .unwrap()
        .set_len(2 * 1024 * 1024)
        .unwrap();
    let mut options = options(
        &root().join("artifacts/original/screen.mkv"),
        &oversized,
        &out.join("result"),
    );
    options.max_event_bytes = 1024;
    let result = analyze::run(options).expect("bounded rejection is structured");
    assert_eq!(result.status, "error");
    let evidence = evidence::read_evidence(&out.join("result/evidence.json")).unwrap();
    let error = evidence.error.expect("diagnostic");
    assert_eq!(error.code, "E_EVENT_BYTES_LIMIT");
    assert_eq!(error.raw_offset, Some(1024));
    assert_eq!(evidence.inputs.events.bytes, 2 * 1024 * 1024);
    fs::remove_dir_all(out).ok();
}

#[test]
fn event_count_limit_reports_the_offending_raw_offset() {
    init_gst();
    let out = temp_dir("count");
    let mut options = options(
        &root().join("artifacts/original/screen.mkv"),
        &root().join("artifacts/original/mouse-events.jsonl"),
        &out.join("result"),
    );
    options.max_events = 10;
    let result = analyze::run(options).expect("structured rejection");
    assert_eq!(result.status, "error");
    let evidence = evidence::read_evidence(&out.join("result/evidence.json")).unwrap();
    let error = evidence.error.expect("diagnostic");
    assert_eq!(error.code, "E_EVENT_COUNT_LIMIT");
    assert_eq!(error.raw_offset, Some(853));
    fs::remove_dir_all(out).ok();
}

#[test]
fn empty_video_has_a_specific_diagnostic() {
    init_gst();
    let out = temp_dir("empty-video");
    let video = out.join("empty.mkv");
    fs::write(&video, b"").unwrap();
    let result = analyze::run(options(
        &video,
        &root().join("artifacts/original/mouse-events.jsonl"),
        &out.join("result"),
    ))
    .expect("empty video is structured");
    assert_eq!(result.status, "error");
    let evidence = evidence::read_evidence(&out.join("result/evidence.json")).unwrap();
    assert_eq!(evidence.error.unwrap().code, "E_EMPTY_VIDEO");
    assert!(evidence.source_unchanged);
    fs::remove_dir_all(out).ok();
}
