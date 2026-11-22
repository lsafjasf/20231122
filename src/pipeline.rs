use std::collections::VecDeque;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use gst::glib::object::Cast;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use serde::Serialize;

use crate::model::EncodeSample;

#[derive(Debug, Clone, Serialize)]
pub struct PipelineConfig {
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub bitrate_kbps: u32,
    pub gop_frames: u32,
    pub duration_ms: u64,
    pub drop_probability: f32,
    pub appsink_max_buffers: u32,
    pub track: u32,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            width: 640,
            height: 360,
            fps_numerator: 30,
            fps_denominator: 1,
            bitrate_kbps: 1_000,
            gop_frames: 15,
            duration_ms: 3_000,
            drop_probability: 0.03,
            appsink_max_buffers: 1,
            track: 0,
        }
    }
}

impl PipelineConfig {
    pub fn extreme() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps_numerator: 240,
            fps_denominator: 1,
            bitrate_kbps: 32,
            gop_frames: 2,
            duration_ms: 2_500,
            drop_probability: 0.25,
            appsink_max_buffers: 1,
            track: 99,
        }
    }

    pub fn description(&self) -> String {
        let fps = self.fps_numerator as f64 / self.fps_denominator as f64;
        let num_buffers = (fps * self.duration_ms as f64 / 1_000.0).ceil() as u64 + 2;
        format!(
            "videotestsrc is-live=true pattern=ball num-buffers={num_buffers} \
             ! video/x-raw,width={width},height={height},framerate={fps_num}/{fps_den} \
             ! x264enc tune=zerolatency speed-preset=ultrafast bframes=0 \
               bitrate={bitrate} key-int-max={gop} \
             ! h264parse \
             ! identity drop-probability={drop} \
             ! appsink name=stats_sink sync=false max-buffers={max_buffers} \
               leaky-type=downstream",
            width = self.width,
            height = self.height,
            fps_num = self.fps_numerator,
            fps_den = self.fps_denominator,
            bitrate = self.bitrate_kbps,
            gop = self.gop_frames,
            drop = self.drop_probability.clamp(0.0, 0.75),
            max_buffers = self.appsink_max_buffers.max(1),
        )
    }

    pub fn nominal_fps(&self) -> f64 {
        self.fps_numerator as f64 / self.fps_denominator.max(1) as f64
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CaptureReport {
    pub config: PipelineConfig,
    pub pipeline_description: String,
    pub gstreamer_version: String,
    pub samples_delivered: u64,
    pub expected_frames: u64,
    pub dropped_frames_detected: u64,
    pub elapsed_ms: u128,
    pub eos: bool,
    pub bus_errors: Vec<String>,
}

pub struct LiveCapture {
    pipeline: gst::Pipeline,
    receiver: Receiver<EncodeSample>,
    config: PipelineConfig,
    description: String,
    started: Instant,
    eos: bool,
    bus_errors: Vec<String>,
}

impl LiveCapture {
    pub fn start(config: PipelineConfig) -> Result<Self> {
        gst::init().context("initialize GStreamer")?;
        let description = config.description();
        let element = gst::parse::launch(&description)
            .with_context(|| format!("parse GStreamer pipeline: {description}"))?;
        let pipeline = element
            .downcast::<gst::Pipeline>()
            .map_err(|_| anyhow!("GStreamer description did not produce a pipeline"))?;
        let sink = pipeline
            .by_name("stats_sink")
            .ok_or_else(|| anyhow!("appsink named stats_sink not found"))?
            .downcast::<gst_app::AppSink>()
            .map_err(|_| anyhow!("stats_sink is not an appsink"))?;

        let (sender, receiver) = mpsc::channel();
        let mut meter = Meter::new(&config);
        sink.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    if let Some(buffer) = sample.buffer() {
                        let stats = meter.observe(buffer);
                        let _ = sender.send(stats);
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        pipeline
            .set_state(gst::State::Playing)
            .context("set GStreamer pipeline to PLAYING")?;
        Ok(Self {
            pipeline,
            receiver,
            config,
            description,
            started: Instant::now(),
            eos: false,
            bus_errors: Vec::new(),
        })
    }

    pub fn receiver(&self) -> &Receiver<EncodeSample> {
        &self.receiver
    }

    pub fn config(&self) -> &PipelineConfig {
        &self.config
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn poll_sample(&mut self) -> Option<EncodeSample> {
        self.receiver.try_recv().ok()
    }

    pub fn pump_bus(&mut self) {
        let Some(bus) = self.pipeline.bus() else {
            return;
        };
        while let Some(message) = bus.pop() {
            use gst::MessageView;
            match message.view() {
                MessageView::Eos(_) => self.eos = true,
                MessageView::Error(error) => {
                    self.bus_errors.push(format!(
                        "{} ({:?}): {}",
                        error.error(),
                        error.src().map(|src| src.path_string()),
                        error.debug().unwrap_or_default()
                    ));
                }
                _ => {}
            }
        }
    }

    pub fn capture_for(&mut self, duration: Duration) -> CaptureReport {
        self.capture_into(duration, |_| {})
    }

    pub fn capture_into<F>(&mut self, duration: Duration, mut on_sample: F) -> CaptureReport
    where
        F: FnMut(EncodeSample),
    {
        let deadline = Instant::now() + duration;
        let mut samples = 0_u64;
        let mut last_dropped = 0_u64;

        while Instant::now() < deadline {
            match self.receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(sample) => {
                    samples = samples.saturating_add(1);
                    last_dropped = sample.stats.dropped_frames;
                    on_sample(sample);
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.pump_bus();
            if self.eos {
                break;
            }
        }
        let duration = Instant::now().saturating_duration_since(self.started);
        let expected_frames = (self.config.nominal_fps() * duration.as_secs_f64()).round() as u64;
        CaptureReport {
            config: self.config.clone(),
            pipeline_description: self.description.clone(),
            gstreamer_version: gst::version_string().to_string(),
            samples_delivered: samples,
            expected_frames,
            dropped_frames_detected: last_dropped,
            elapsed_ms: duration.as_millis(),
            eos: self.eos,
            bus_errors: self.bus_errors.clone(),
        }
    }

    pub fn stop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl Drop for LiveCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Meter {
    track: u32,
    nominal_fps: f64,
    nominal_period_ns: f64,
    first_pts: Option<u64>,
    last_pts: Option<u64>,
    expected_frame: Option<u64>,
    dropped_total: u64,
    encoded_bytes: u64,
    window: VecDeque<(u64, u64)>,
}

impl Meter {
    fn new(config: &PipelineConfig) -> Self {
        Self {
            track: config.track,
            nominal_fps: config.nominal_fps().max(0.001),
            nominal_period_ns: 1_000_000_000.0 / config.nominal_fps().max(0.001),
            first_pts: None,
            last_pts: None,
            expected_frame: None,
            dropped_total: 0,
            encoded_bytes: 0,
            window: VecDeque::new(),
        }
    }

    fn observe(&mut self, buffer: &gst::BufferRef) -> EncodeSample {
        let pts = buffer
            .pts()
            .map(|value| value.nseconds())
            .or_else(|| buffer.dts().map(|value| value.nseconds()))
            .unwrap_or(0);
        let size = buffer.size() as u64;
        self.encoded_bytes = self.encoded_bytes.saturating_add(size);
        let first_pts = *self.first_pts.get_or_insert(pts);

        let frame = ((pts.saturating_sub(first_pts) as f64 / self.nominal_period_ns).round()
            as u64)
            .saturating_add(1);
        if let Some(expected) = self.expected_frame
            && frame > expected.saturating_add(1)
        {
            self.dropped_total = self.dropped_total.saturating_add(frame - expected - 1);
        }
        self.expected_frame = Some(frame);

        let fps = if let Some(last_pts) = self.last_pts {
            let delta = pts.saturating_sub(last_pts);
            if delta == 0 {
                self.nominal_fps
            } else {
                (self.nominal_period_ns / delta as f64 * self.nominal_fps).clamp(0.001, 1_000_000.0)
            }
        } else {
            self.nominal_fps
        };

        self.window.push_back((pts, size));
        let window_start = pts.saturating_sub(1_000_000_000);
        while self
            .window
            .front()
            .is_some_and(|(time, _)| *time < window_start)
        {
            self.window.pop_front();
        }
        let bytes: u64 = self.window.iter().map(|(_, bytes)| *bytes).sum();
        let measured_span_ns = self
            .window
            .front()
            .map(|(start, _)| pts.saturating_sub(*start))
            .unwrap_or(0);
        let span_ns = measured_span_ns.max(self.nominal_period_ns as u64).max(1);
        let bitrate_bps =
            ((bytes as u128 * 8 * 1_000_000_000) / span_ns as u128).min(u64::MAX as u128) as u64;

        self.last_pts = Some(pts);
        EncodeSample::new(
            pts,
            self.track,
            frame,
            fps,
            bitrate_bps,
            self.dropped_total,
            self.encoded_bytes,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_description_has_all_required_stages() {
        let description = PipelineConfig::default().description();
        for stage in ["videotestsrc", "x264enc", "h264parse", "appsink"] {
            assert!(
                description.contains(stage),
                "missing {stage}: {description}"
            );
        }
    }

    #[test]
    fn extreme_description_is_still_finite() {
        let config = PipelineConfig::extreme();
        let description = config.description();
        assert!(description.contains("framerate=240/1"));
        assert!(description.contains("drop-probability=0.25"));
        assert!(!description.contains("NaN"));
    }
}
