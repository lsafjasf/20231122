use gstreamer as gst;
use gst::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::path::{Path, PathBuf};

/// Set by the GTK4 UI "中途取消" button; polled by `export`.
pub static MANUAL_CANCEL: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSpec {
    pub input_raw: PathBuf,
    pub output_wav: PathBuf,
    pub rate: u32,
    pub channels: u32,
    pub dithering: String,
    pub noise_shaping: String,
    /// If set, abort the pipeline (transition to NULL, no EOS) at this position.
    pub cancel_at_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOutcome {
    pub final_state: String,
    pub cancelled: bool,
    pub position_at_end_ns: Option<u64>,
    pub bytes_written: u64,
    pub header_sane: bool,
    pub sha256: String,
}

pub fn init() -> Result<(), String> {
    gst::init().map_err(|e| format!("gst::init failed: {e}"))
}

pub fn export(spec: &ExportSpec) -> Result<ExportOutcome, String> {
    let src = gst::ElementFactory::make("filesrc")
        .property("location", spec.input_raw.to_string_lossy().as_ref())
        .build()
        .map_err(|e| e.to_string())?;
    let parse = gst::ElementFactory::make("rawaudioparse")
        .property("sample-rate", spec.rate as i32)
        .property("num-channels", spec.channels as i32)
        .build()
        .map_err(|e| e.to_string())?;
    parse.set_property_from_str("format", "pcm");
    parse.set_property_from_str("pcm-format", "f32le");
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(|e| e.to_string())?;
    convert.set_property_from_str("dithering", &spec.dithering);
    convert.set_property_from_str("noise-shaping", &spec.noise_shaping);
    let caps = gst::ElementFactory::make("capsfilter")
        .property(
            "caps",
            gst::Caps::builder("audio/x-raw")
                .field("format", "S16LE")
                .field("rate", spec.rate as i32)
                .field("channels", spec.channels as i32)
                .build(),
        )
        .build()
        .map_err(|e| e.to_string())?;
    // Pace the pipeline to realtime so a mid-way cancel lands at a
    // deterministic stream position instead of racing the 20 ms poll loop.
    let pace = gst::ElementFactory::make("identity")
        .property("sync", true)
        .build()
        .map_err(|e| e.to_string())?;
    let enc = gst::ElementFactory::make("wavenc")
        .build()
        .map_err(|e| e.to_string())?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", spec.output_wav.to_string_lossy().as_ref())
        .build()
        .map_err(|e| e.to_string())?;

    let pipe = gst::Pipeline::new();
    pipe.add_many([&src, &parse, &convert, &caps, &pace, &enc, &sink])
        .map_err(|e| e.to_string())?;
    gst::Element::link_many([&src, &parse, &convert, &caps, &pace, &enc, &sink])
        .map_err(|e| e.to_string())?;

    pipe.set_state(gst::State::Playing)
        .map_err(|e| format!("set PLAYING: {e}"))?;

    let mut cancelled = false;
    let mut position_at_end = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if std::time::Instant::now() > deadline {
            pipe.set_state(gst::State::Null).ok();
            return Err("export timed out after 60s".into());
        }
        let pos = pipe.query_position::<gst::ClockTime>();
        if MANUAL_CANCEL.swap(false, Ordering::SeqCst) && !cancelled {
            position_at_end = pos.map(|p| p.nseconds());
            pipe.set_state(gst::State::Null)
                .map_err(|e| format!("manual cancel transition: {e}"))?;
            cancelled = true;
            break;
        }
        if let Some(cancel_at) = spec.cancel_at_ns {
            if let Some(p) = pos {
                if p.nseconds() >= cancel_at && !cancelled {
                    position_at_end = Some(p.nseconds());
                    pipe.set_state(gst::State::Null)
                        .map_err(|e| format!("cancel transition: {e}"))?;
                    cancelled = true;
                    break;
                }
            }
        }
        let bus = pipe.bus();
        if let Some(bus) = bus {
            if let Some(msg) = bus.timed_pop(gst::ClockTime::from_mseconds(20)) {
                use gst::MessageView;
                match msg.view() {
                    MessageView::Eos(_) => break,
                    MessageView::Error(err) => {
                        pipe.set_state(gst::State::Null).ok();
                        return Err(format!("pipeline error: {}", err.error()));
                    }
                    _ => {}
                }
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    if !cancelled {
        pipe.set_state(gst::State::Null).ok();
    }
    let final_state = format!("{:?}", pipe.current_state());

    let bytes_written = std::fs::metadata(&spec.output_wav)
        .map(|m| m.len())
        .unwrap_or(0);
    let header_sane = wav_header_sane(&spec.output_wav);
    let sha256 = crate::analysis::sha256_file(&spec.output_wav)?;
    Ok(ExportOutcome {
        final_state,
        cancelled,
        position_at_end_ns: position_at_end,
        bytes_written,
        header_sane,
        sha256,
    })
}

/// A finalized WAV has RIFF size == filesize - 8; an aborted wavenc leaves placeholders.
fn wav_header_sane(path: &Path) -> bool {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(_) => return false,
    };
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return false;
    }
    let riff_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as u64;
    riff_size == data.len() as u64 - 8
}

pub fn record_gst_env(dir: &Path) -> Result<PathBuf, String> {
    init()?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut text = String::new();
    text.push_str(&format!("gstreamer version: {}\n", gst::version_string()));
    for name in ["audioconvert", "wavenc", "rawaudioparse", "filesink"] {
        match gst::ElementFactory::find(name) {
            Some(f) => {
                text.push_str(&format!(
                    "element {name}: present (klass={}, description={})\n",
                    f.metadata(gst::ELEMENT_METADATA_KLASS).unwrap_or("?"),
                    f.metadata(gst::ELEMENT_METADATA_DESCRIPTION).unwrap_or("?"),
                ));
            }
            None => text.push_str(&format!("element {name}: MISSING\n")),
        }
    }
    if let Ok(ac) = gst::ElementFactory::make("audioconvert").build() {
        for prop in ac.list_properties() {
            if prop.name().contains("dither") || prop.name().contains("noise") {
                text.push_str(&format!(
                    "audioconvert property: {} ({})\n",
                    prop.name(),
                    prop.blurb().unwrap_or_default()
                ));
            }
        }
    }
    text.push_str(&format!(
        "GST_PLUGIN_PATH={}\n",
        std::env::var("GST_PLUGIN_PATH").unwrap_or_default()
    ));
    let path = dir.join("gst-env.txt");
    std::fs::write(&path, text).map_err(|e| e.to_string())?;
    Ok(path)
}
