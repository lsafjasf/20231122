//! Synthetic sample media: one Matroska file with a video track and two
//! dubbing tracks ("eng" 440 Hz sine, "zho" 550 Hz sine), each tagged with
//! its language code. Also a small duration probe used to plan segments.

use std::path::Path;

use anyhow::Context;
use gstreamer::prelude::*;

pub const SAMPLE_DURATION_MS: u64 = 6_000;

/// Normalise a language tag for comparison.
///
/// Containers report ISO-639-1 ("en") while users tend to type ISO-639-2
/// ("eng"); both map to the same canonical 2-letter form here. Unknown
/// tags are lowercased and compared verbatim.
pub fn normalize_language(tag: &str) -> String {
    let t = tag.trim().to_lowercase();
    match t.as_str() {
        "eng" => "en",
        "zho" | "chi" => "zh",
        "jpn" => "ja",
        "kor" => "ko",
        "fra" | "fre" => "fr",
        "deu" | "ger" => "de",
        "spa" => "es",
        "por" => "pt",
        "rus" => "ru",
        "ita" => "it",
        "ara" => "ar",
        "hin" => "hi",
        other => return other.to_string(),
    }
    .to_string()
}

fn run_to_end(pipeline: &gstreamer::Pipeline) -> anyhow::Result<()> {
    pipeline.set_state(gstreamer::State::Playing)?;
    let bus = pipeline.bus().context("pipeline has no bus")?;
    for msg in bus.iter_timed(gstreamer::ClockTime::from_seconds(60)) {
        use gstreamer::MessageView;
        match msg.view() {
            MessageView::Eos(_) => break,
            MessageView::Error(e) => {
                pipeline.set_state(gstreamer::State::Null).ok();
                anyhow::bail!("{}: {}", e.error(), e.debug().unwrap_or_default());
            }
            _ => {}
        }
    }
    pipeline.set_state(gstreamer::State::Null)?;
    Ok(())
}

/// Generate the sample file. Idempotent: existing files are overwritten.
pub fn make_sample(out: &Path) -> anyhow::Result<()> {
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // 6 s @ 48 kHz mono per audio track; 150 video frames @ 25 fps.
    let desc = format!(
        "matroskamux name=mux ! filesink location={out} \
         videotestsrc num-buffers=150 ! video/x-raw,framerate=25/1,width=320,height=240 \
           ! vp8enc deadline=1000000 ! mux.video_0 \
         audiotestsrc wave=sine freq=440 samplesperbuffer=480 num-buffers=600 \
           ! audio/x-raw,rate=48000,channels=1 ! opusenc \
           ! taginject tags=\"language-code=eng,title=English\" ! mux.audio_0 \
         audiotestsrc wave=sine freq=550 samplesperbuffer=480 num-buffers=600 \
           ! audio/x-raw,rate=48000,channels=1 ! opusenc \
           ! taginject tags=\"language-code=zho,title=Chinese\" ! mux.audio_1",
        out = out.display()
    );
    let pipeline = gstreamer::parse::launch(&desc)?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow::anyhow!("not a pipeline"))?;
    run_to_end(&pipeline)
}

/// Container duration in milliseconds, probed with a throwaway pipeline.
pub fn duration_ms(input: &Path) -> anyhow::Result<u64> {
    let desc = format!(
        "filesrc location={} ! matroskademux ! fakesink",
        input.display()
    );
    let pipeline = gstreamer::parse::launch(&desc)?
        .downcast::<gstreamer::Pipeline>()
        .map_err(|_| anyhow::anyhow!("not a pipeline"))?;
    pipeline.set_state(gstreamer::State::Paused)?;
    let _ = pipeline.state(gstreamer::ClockTime::from_seconds(10));
    let dur = pipeline
        .query_duration::<gstreamer::ClockTime>()
        .map(|d| d.mseconds())
        .unwrap_or(SAMPLE_DURATION_MS);
    pipeline.set_state(gstreamer::State::Null)?;
    Ok(dur)
}

/// Discover audio dubbing tracks: pad name -> language code.
///
/// Runs `filesrc ! matroskademux` (linked to fakesinks as pads appear)
/// until `no-more-pads`, then reads the sticky tag events on each audio
/// src pad — no decoding is needed.
pub fn list_audio_tracks(input: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let src = gstreamer::ElementFactory::make("filesrc")
        .property("location", input.to_string_lossy().as_ref())
        .build()?;
    let demux = gstreamer::ElementFactory::make("matroskademux").build()?;
    let pipeline = gstreamer::Pipeline::new();
    pipeline.add_many([&src, &demux])?;
    src.link(&demux)?;
    // Link audio pads to fakesinks so the pipeline can preroll.
    let demux_weak = demux.downgrade();
    let pipeline_weak = pipeline.downgrade();
    demux.connect_pad_added(move |_, pad| {
        let Some(demux) = demux_weak.upgrade() else {
            return;
        };
        let Some(pipeline) = pipeline_weak.upgrade() else {
            return;
        };
        let caps = pad.current_caps().or_else(|| Some(pad.query_caps(None)));
        let is_audio = caps
            .and_then(|c| c.structure(0).map(|s| s.name().starts_with("audio/")))
            .unwrap_or(false);
        if !is_audio {
            return;
        }
        if let Ok(sink) = gstreamer::ElementFactory::make("fakesink").build() {
            if pipeline.add(&sink).is_ok() {
                sink.sync_state_with_parent().ok();
                if let Some(sinkpad) = sink.static_pad("sink") {
                    let _ = pad.link(&sinkpad);
                }
            }
        }
        drop(demux);
    });
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let done2 = done.clone();
    demux.connect_no_more_pads(move |_| {
        done2.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    pipeline.set_state(gstreamer::State::Playing)?;
    // Wait for the demuxer to expose all pads (bounded).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !done.load(std::sync::atomic::Ordering::SeqCst) {
        anyhow::ensure!(
            std::time::Instant::now() < deadline,
            "timed out waiting for demuxer pads"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let mut tracks = Vec::new();
    for pad in demux.src_pads() {
        let name = pad.name().to_string();
        if !name.starts_with("audio") {
            continue;
        }
        let mut language = String::from("und");
        pad.sticky_events_foreach(|event| {
            if let gstreamer::EventView::Tag(tags) = event.view() {
                let taglist = tags.tag();
                if let Some(code) = taglist.get::<gstreamer::tags::LanguageCode>() {
                    language = code.get().to_string();
                }
            }
            std::ops::ControlFlow::Continue(gstreamer::EventForeachAction::Keep)
        });
        tracks.push((name, language));
    }
    tracks.sort();
    pipeline.set_state(gstreamer::State::Null)?;
    Ok(tracks)
}
