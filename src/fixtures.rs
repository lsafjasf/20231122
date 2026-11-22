use anyhow::{bail, Context, Result};
use gstreamer as gst;
use gstreamer::glib::object::Cast;
use gstreamer::prelude::*;
use std::fs;
use std::path::Path;

use crate::util::{atomic_write, now_iso};

pub fn ensure_demo(root: &Path) -> Result<()> {
    let project = root.join("demo_project");
    let media = project.join("media");
    let capture = project.join("capture");
    fs::create_dir_all(&media)?;
    fs::create_dir_all(&capture)?;

    let intro = media.join("intro_master.mp4");
    if !intro.exists() {
        render_video(&intro, "smpte", 144)?;
    }
    let recovered = media.join("recovered_take_07.mp4");
    if !recovered.exists() {
        render_video(&recovered, "ball", 96)?;
    }
    let roomtone = media.join("roomtone_48k.wav");
    if !roomtone.exists() {
        render_audio(&roomtone)?;
    }

    let unknown = capture.join("swing_notes.capx");
    if !unknown.exists() {
        let body = concat!(
            "FIELD CAPTURE / SWING TAKE 07\n",
            "device=tablet-02\n",
            "start=16:42:11+08:00\n",
            "operator=lin\n",
            "note=audio slate at 00:03; camera B card was swapped\n",
            "sha_hint=not-provided\n"
        );
        atomic_write(&unknown, body.as_bytes())?;
    }

    let orphan = capture.join("orphan_tail.bin");
    if !orphan.exists() {
        let mut bytes = fs::read(&intro).context("读取 MP4 以构造截断边界样本")?;
        let keep = bytes.len().min(1536);
        bytes.truncate(keep);
        atomic_write(&orphan, &bytes)?;
    }

    let marker = root.join("artifacts/runtime/fixtures.json");
    let marker_body = serde_json::json!({
        "generated_at": now_iso(),
        "project": "demo_project",
        "note": "Media is rendered locally with GStreamer; missing source is intentionally absent.",
        "pipeline": "videotestsrc/audiotestsrc -> encoder -> muxer -> filesink"
    });
    crate::util::write_json(&marker, &marker_body)?;
    Ok(())
}

fn render_video(path: &Path, pattern: &str, frames: u64) -> Result<()> {
    let target = path.display();
    let description = format!(
        "videotestsrc num-buffers={frames} pattern={pattern} ! \
         video/x-raw,width=640,height=360,framerate=24/1 ! \
         jpegenc quality=82 ! qtmux ! filesink location=\"{target}\""
    );
    run_pipeline(&description).with_context(|| format!("生成测试视频 {}", path.display()))
}

fn render_audio(path: &Path) -> Result<()> {
    let target = path.display();
    let description = format!(
        "audiotestsrc num-buffers=144 wave=sine freq=440 ! \
         audioconvert ! audioresample ! \
         audio/x-raw,rate=48000,channels=1 ! wavenc ! filesink location=\"{target}\""
    );
    run_pipeline(&description).with_context(|| format!("生成测试音频 {}", path.display()))
}

fn run_pipeline(description: &str) -> Result<()> {
    gst::init()?;
    let pipeline = gst::parse::launch(description)?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("GStreamer 描述未生成管线"))?;
    pipeline.set_state(gst::State::Playing)?;
    let bus = pipeline.bus().context("GStreamer 管线没有 bus")?;
    let message = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(30),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    pipeline.set_state(gst::State::Null)?;
    match message {
        Some(message) => match message.view() {
            gst::MessageView::Eos(_) => Ok(()),
            gst::MessageView::Error(error) => {
                bail!(
                    "GStreamer: {} ({:?})",
                    error.error(),
                    error.debug().map(|value| value.to_string())
                )
            }
            _ => bail!("GStreamer 返回了意外消息"),
        },
        None => bail!("GStreamer 管线超时"),
    }
}
