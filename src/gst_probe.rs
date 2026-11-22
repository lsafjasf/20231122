use serde::{Deserialize, Serialize};

#[cfg(not(feature = "gst"))]
use crate::model::FrameRecord;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedBuffer {
    pub pts_ns: u64,
    pub dts_ns: u64,
    pub size: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GstProbeReport {
    pub initialized: bool,
    pub version: String,
    pub pipeline: String,
    pub appsrc_available: bool,
    pub fakesink_available: bool,
    pub expected_frames: usize,
    pub observed_frames: usize,
    pub timestamps_preserved: bool,
    pub observed: Vec<ObservedBuffer>,
    pub error: Option<String>,
}

impl GstProbeReport {
    pub fn passed(&self) -> bool {
        self.initialized
            && self.appsrc_available
            && self.fakesink_available
            && self.expected_frames == self.observed_frames
            && self.timestamps_preserved
            && self.error.is_none()
    }
}

#[cfg(feature = "gst")]
pub use enabled::probe_roundtrip;

#[cfg(not(feature = "gst"))]
pub fn probe_roundtrip(frames: &[FrameRecord]) -> GstProbeReport {
    GstProbeReport {
        initialized: false,
        version: "feature disabled".to_string(),
        pipeline: "appsrc ! fakesink".to_string(),
        appsrc_available: false,
        fakesink_available: false,
        expected_frames: frames.len(),
        observed_frames: 0,
        timestamps_preserved: false,
        observed: Vec::new(),
        error: Some("the `gst` cargo feature is disabled".to_string()),
    }
}

#[cfg(feature = "gst")]
mod enabled {
    use std::sync::{Arc, Mutex, OnceLock};

    use gstreamer as gst;
    use gstreamer::prelude::*;

    use super::{GstProbeReport, ObservedBuffer};
    use crate::model::FrameRecord;

    static INIT: OnceLock<Result<(), String>> = OnceLock::new();

    pub fn probe_roundtrip(frames: &[FrameRecord]) -> GstProbeReport {
        let mut report = GstProbeReport {
            initialized: false,
            version: gst::version_string().to_string(),
            pipeline: "appsrc ! fakesink".to_string(),
            appsrc_available: false,
            fakesink_available: false,
            expected_frames: frames.len(),
            observed_frames: 0,
            timestamps_preserved: false,
            observed: Vec::new(),
            error: None,
        };

        let init = INIT.get_or_init(|| gst::init().map_err(|error| error.to_string()));
        if let Err(error) = init {
            report.error = Some(error.clone());
            return report;
        }
        report.initialized = true;

        let appsrc_available = gst::ElementFactory::find("appsrc").is_some();
        let fakesink_available = gst::ElementFactory::find("fakesink").is_some();
        report.appsrc_available = appsrc_available;
        report.fakesink_available = fakesink_available;
        if !appsrc_available || !fakesink_available {
            report.error = Some("GStreamer app/coreelements plugins are not available".to_string());
            return report;
        }

        match run_pipeline(frames) {
            Ok(observed) => {
                report.timestamps_preserved =
                    observed.iter().zip(frames).all(|(actual, expected)| {
                        actual.pts_ns == expected.pts_ns && actual.dts_ns == expected.dts_ns
                    });
                report.observed_frames = observed.len();
                report.observed = observed;
            }
            Err(error) => report.error = Some(error),
        }
        report
    }

    fn run_pipeline(frames: &[FrameRecord]) -> Result<Vec<ObservedBuffer>, String> {
        let pipeline = gst::Pipeline::new();
        let appsrc = gst::ElementFactory::make("appsrc")
            .name("bframe-records")
            .build()
            .map_err(|error| error.to_string())?
            .downcast::<gstreamer_app::AppSrc>()
            .map_err(|_| "appsrc downcast failed".to_string())?;
        let fakesink = gst::ElementFactory::make("fakesink")
            .name("evidence-sink")
            .build()
            .map_err(|error| error.to_string())?;
        fakesink.set_property("sync", false);
        fakesink.set_property("silent", true);

        pipeline
            .add_many([appsrc.upcast_ref::<gst::Element>(), &fakesink])
            .map_err(|error| error.to_string())?;
        gst::Element::link_many([appsrc.upcast_ref::<gst::Element>(), &fakesink])
            .map_err(|error| error.to_string())?;

        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "I420")
            .field("width", 16i32)
            .field("height", 16i32)
            .field("framerate", gst::Fraction::new(1, 1))
            .build();
        appsrc.set_caps(Some(&caps));
        appsrc.set_format(gst::Format::Time);
        appsrc.set_is_live(false);

        let observed = Arc::new(Mutex::new(Vec::new()));
        let sink_pad = fakesink
            .static_pad("sink")
            .ok_or_else(|| "fakesink has no sink pad".to_string())?;
        let probe_data = Arc::clone(&observed);
        sink_pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(buffer) = info.buffer() {
                let mut values = probe_data.lock().expect("probe lock poisoned");
                values.push(ObservedBuffer {
                    pts_ns: buffer.pts().map(|time| time.nseconds()).unwrap_or(0),
                    dts_ns: buffer.dts().map(|time| time.nseconds()).unwrap_or(0),
                    size: buffer.size(),
                });
            }
            gst::PadProbeReturn::Ok
        });

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|error| error.to_string())?;
        for frame in frames {
            let mut buffer = gst::Buffer::from_slice(vec![0u8; 384]);
            {
                let buffer = buffer.get_mut().expect("fresh buffer is writable");
                buffer.set_pts(gst::ClockTime::from_nseconds(frame.pts_ns));
                buffer.set_dts(gst::ClockTime::from_nseconds(frame.dts_ns));
            }
            appsrc
                .push_buffer(buffer)
                .map_err(|error| error.to_string())?;
        }
        appsrc.end_of_stream().map_err(|error| error.to_string())?;

        let bus = pipeline
            .bus()
            .ok_or_else(|| "pipeline has no bus".to_string())?;
        let message = bus.timed_pop_filtered(
            gst::ClockTime::from_seconds(5),
            &[gst::MessageType::Eos, gst::MessageType::Error],
        );
        let _ = pipeline.set_state(gst::State::Null);
        match message {
            Some(message) if message.type_() == gst::MessageType::Eos => {}
            Some(message) => {
                if let gst::MessageView::Error(error) = message.view() {
                    return Err(format!(
                        "pipeline error from {}: {}",
                        error.src().map(|src| src.path_string()).unwrap_or_default(),
                        error.error()
                    ));
                }
                return Err(format!("unexpected pipeline message {:?}", message.type_()));
            }
            None => return Err("EOS timeout after 5 seconds".to_string()),
        }

        let observed = observed
            .lock()
            .map_err(|_| "probe lock poisoned".to_string())?
            .clone();
        Ok(observed)
    }
}
