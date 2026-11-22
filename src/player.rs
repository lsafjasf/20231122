use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result};
use gst::prelude::*;
use gst_app::AppSink;

use crate::mask::{draw_mask, MaskConfig};
use crate::media::ensure_gstreamer;

#[derive(Debug, Clone)]
pub struct FrameData {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
pub struct FrameMailbox {
    latest: Mutex<Option<FrameData>>,
}

impl FrameMailbox {
    pub fn push(&self, frame: FrameData) {
        if let Ok(mut latest) = self.latest.lock() {
            *latest = Some(frame);
        }
    }

    pub fn take_latest(&self) -> Option<FrameData> {
        self.latest.lock().ok()?.take()
    }
}

pub struct PreviewPlayer {
    pipeline: gst::Pipeline,
    sink: AppSink,
    duration_ns: u64,
    last_seek_ns: Arc<Mutex<u64>>,
    error: Arc<Mutex<Option<String>>>,
}

impl PreviewPlayer {
    pub fn open(
        path: &Path,
        duration_ns: u64,
        mask: Arc<RwLock<MaskConfig>>,
        mailbox: Arc<FrameMailbox>,
    ) -> Result<Self> {
        ensure_gstreamer()?;
        let pipeline = gst::Pipeline::new();
        let source = make("filesrc")?;
        source.set_property("location", path.to_string_lossy().as_ref());
        let decode = make("decodebin")?;
        let convert_in = make("videoconvert")?;
        let overlay = make("cairooverlay")?;
        let convert_out = make("videoconvert")?;
        let caps_filter = make("capsfilter")?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .build();
        caps_filter.set_property("caps", &caps);
        let sink = make("appsink")?
            .downcast::<AppSink>()
            .map_err(|_| anyhow::anyhow!("appsink 类型转换失败"))?;
        sink.set_max_buffers(1);
        sink.set_drop(true);
        sink.set_sync(true);

        pipeline.add_many([
            &source,
            &decode,
            &convert_in,
            &overlay,
            &convert_out,
            &caps_filter,
            sink.upcast_ref(),
        ])?;
        source.link(&decode)?;
        convert_in.link(&overlay)?;
        overlay.link(&convert_out)?;
        convert_out.link(&caps_filter)?;
        caps_filter.link(&sink)?;

        {
            let convert_in = convert_in.clone();
            decode.connect_pad_added(move |_, pad| {
                let is_video = pad
                    .current_caps()
                    .or_else(|| Some(pad.query_caps(None)))
                    .and_then(|caps| {
                        caps.structure(0)
                            .map(|structure| structure.name().to_string())
                    })
                    .map(|name| name.starts_with("video/"))
                    .unwrap_or(false);
                if is_video {
                    if let Some(sink_pad) = convert_in.static_pad("sink") {
                        if !sink_pad.is_linked() {
                            let _ = pad.link(&sink_pad);
                        }
                    }
                }
            });
        }

        connect_mask_draw(&overlay, mask);

        let mailbox_for_callback = Arc::clone(&mailbox);
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let Some(sample) = sink.pull_sample().ok() else {
                        return Err(gst::FlowError::Error);
                    };
                    let Some(caps) = sample.caps() else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let info = match gst_video::VideoInfo::from_caps(caps) {
                        Ok(info) => info,
                        Err(_) => return Ok(gst::FlowSuccess::Ok),
                    };
                    let Some(buffer) = sample.buffer() else {
                        return Ok(gst::FlowSuccess::Ok);
                    };
                    let Ok(map) = buffer.map_readable() else {
                        return Err(gst::FlowError::Error);
                    };
                    let stride = info.stride().first().copied().unwrap_or(0).max(0) as usize;
                    mailbox_for_callback.push(FrameData {
                        width: info.width(),
                        height: info.height(),
                        stride,
                        bytes: map.as_slice().to_vec(),
                    });
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        let error = Arc::new(Mutex::new(None));
        if let Some(bus) = pipeline.bus() {
            let error_slot = Arc::clone(&error);
            let _ = bus.add_watch_local(move |_, message| {
                if let gst::MessageView::Error(err) = message.view() {
                    if let Ok(mut slot) = error_slot.lock() {
                        *slot = Some(format!(
                            "{} ({})",
                            err.error(),
                            err.debug().unwrap_or_default()
                        ));
                    }
                }
                gst::glib::ControlFlow::Continue
            });
        }

        let last_seek_ns = Arc::new(Mutex::new(0_u64));
        pipeline.set_state(gst::State::Paused)?;
        Ok(Self {
            pipeline,
            sink,
            duration_ns,
            last_seek_ns,
            error,
        })
    }

    pub fn play(&self) -> Result<()> {
        self.pipeline.set_state(gst::State::Playing)?;
        Ok(())
    }

    pub fn pause(&self) -> Result<()> {
        self.pipeline.set_state(gst::State::Paused)?;
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        self.pipeline.current_state() == gst::State::Playing
    }

    pub fn seek_ns(&self, ns: u64) -> Result<()> {
        let target = ns.min(self.duration_ns.saturating_sub(1));
        if let Ok(mut last) = self.last_seek_ns.lock() {
            *last = target;
        }
        self.pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::ClockTime::from_nseconds(target),
        )?;
        Ok(())
    }

    pub fn position_ns(&self) -> u64 {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map(|time| time.nseconds())
            .or_else(|| self.last_seek_ns.lock().ok().map(|value| *value))
            .unwrap_or(0)
    }

    pub fn take_error(&self) -> Option<String> {
        self.error.lock().ok()?.take()
    }

    pub fn stop(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl Drop for PreviewPlayer {
    fn drop(&mut self) {
        self.sink
            .set_callbacks(gst_app::AppSinkCallbacks::builder().build());
        self.stop();
    }
}

pub fn connect_mask_draw(overlay: &gst::Element, mask: Arc<RwLock<MaskConfig>>) {
    overlay.connect("draw", false, move |values| {
        let context = values
            .get(1)
            .and_then(|value| value.get::<cairo::Context>().ok())?;
        let config = mask.read().map(|config| config.clone()).unwrap_or_default();
        let (width, height) = context
            .clip_extents()
            .map(|(_, _, width, height)| (width, height))
            .unwrap_or((1920.0, 1080.0));
        if width > 0.0 && height > 0.0 {
            draw_mask(&context, &config, width, height, "");
        }
        None
    });
}

fn make(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .build()
        .with_context(|| format!("缺少 GStreamer 元素 {factory}"))
}
