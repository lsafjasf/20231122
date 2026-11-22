//! GStreamer 集成测试：caps 解析、真实管线提取、文件往返。

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_video as gst_video;
use hdr_meta_fix::gst as hgst;
use hdr_meta_fix::meta::*;

fn init() {
    let _ = gst::init();
}

#[test]
fn caps_with_hdr10_metadata_parse_correctly() {
    init();
    use std::str::FromStr;
    let mut caps = gst::Caps::from_str(
        "video/x-raw,format=I420,width=16,height=16,colorimetry=bt2100-pq,range=limited",
    )
    .unwrap();
    let md = gst_video::VideoMasteringDisplayInfo::new(
        [
            gst_video::VideoMasteringDisplayInfoCoordinate::new(0.68, 0.32),
            gst_video::VideoMasteringDisplayInfoCoordinate::new(0.265, 0.69),
            gst_video::VideoMasteringDisplayInfoCoordinate::new(0.15, 0.06),
        ],
        gst_video::VideoMasteringDisplayInfoCoordinate::new(0.3127, 0.329),
        1000,
        1,
    );
    md.add_to_caps(caps.make_mut());
    let cll = gst_video::VideoContentLightLevel::new(800, 400);
    cll.add_to_caps(caps.make_mut());

    let meta = hgst::frame_meta_from_caps(&caps, 0);
    assert_eq!(
        meta.get(FieldKey::Transfer),
        Some(FieldValue::Transfer(Transfer::Pq))
    );
    assert_eq!(
        meta.get(FieldKey::Primaries),
        Some(FieldValue::Primaries(Primaries::Bt2020))
    );
    assert_eq!(
        meta.get(FieldKey::HdrFormat),
        Some(FieldValue::Format(HdrFormat::Hdr10)),
        "PQ 应推断出 HDR10 标签"
    );
    assert_eq!(
        meta.get(FieldKey::MasteringMaxLum),
        Some(FieldValue::Number(1000.0))
    );
    assert_eq!(meta.get(FieldKey::MaxCll), Some(FieldValue::Number(800.0)));
    assert_eq!(meta.get(FieldKey::MaxFall), Some(FieldValue::Number(400.0)));
}

#[test]
fn pipeline_videotestsrc_colorimetry_extracted() {
    init();
    let pipeline = gst::parse::launch(
        "videotestsrc num-buffers=3 ! video/x-raw,colorimetry=bt2100-pq ! \
         appsink name=sink",
    )
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap();
    let sink = pipeline.by_name("sink").unwrap().downcast::<AppSink>().unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    let sample = sink.pull_sample().unwrap();
    let caps = sample.caps().unwrap();
    let meta = hgst::frame_meta_from_caps(caps, 0);
    pipeline.set_state(gst::State::Null).unwrap();
    assert_eq!(
        meta.get(FieldKey::Transfer),
        Some(FieldValue::Transfer(Transfer::Pq))
    );
}

#[test]
fn extract_frames_from_encoded_file() {
    init();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    let pipeline = gst::parse::launch(&format!(
        "videotestsrc num-buffers=10 ! video/x-raw,format=I420 ! \
         openh264enc ! h264parse ! mp4mux ! filesink location={}",
        path.display()
    ))
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    let bus = pipeline.bus().unwrap();
    for msg in bus.iter_timed(gst::ClockTime::from_seconds(30)) {
        if matches!(msg.view(), gst::MessageView::Eos(_) | gst::MessageView::Error(_)) {
            break;
        }
    }
    pipeline.set_state(gst::State::Null).unwrap();

    let frames = hgst::extract_frames(&path.to_string_lossy(), 50).unwrap();
    assert_eq!(frames.len(), 10, "应提取 10 帧，实际 {}", frames.len());
    // SDR 素材：帧号必须连续。
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f.frame_no, i as u64);
    }
}
