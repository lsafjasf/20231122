use crate::model::FrameState;
#[cfg(feature = "gstreamer")]
use crate::replay::FrameProcessor;
use crate::replay::{ReplayReport, WavePoint};
use anyhow::{Context, Result};
#[cfg(feature = "gstreamer")]
use anyhow::{anyhow, bail};
#[cfg(feature = "gstreamer")]
use gstreamer::prelude::ElementExt;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

pub fn write_waveform_csv(path: &Path, points: &[WavePoint]) -> Result<()> {
    let mut output = String::from(
        "sample,time_ms,pts_ns,track_id,track_index,frame,source_state,output_luma,measured_luma,confidence,compensation_mode,params_fingerprint,record_id,evidence_sha256\n",
    );
    for point in points {
        writeln!(
            output,
            "{},{},{},{},{},{},{},{:.6},{:.6},{:.6},{},{},{},{}",
            point.sample,
            point.time_ms,
            point.pts_ns,
            point.track_id,
            point.track_index,
            point.frame,
            state_name(point.source_state),
            point.output_luma,
            point.measured_luma,
            point.confidence,
            point.compensation_mode,
            point.params_fingerprint,
            point.record_id,
            point.evidence_sha256
        )?;
    }
    fs::write(path, output).with_context(|| format!("write {}", path.display()))
}

fn state_name(state: FrameState) -> &'static str {
    match state {
        FrameState::Present => "present",
        FrameState::Missing => "missing",
    }
}

pub fn write_waveform_svg(
    path: &Path,
    report: &ReplayReport,
    title: &str,
    reconstructed: bool,
) -> Result<()> {
    fs::write(path, waveform_svg(report, title, reconstructed))
        .with_context(|| format!("write {}", path.display()))
}

pub fn waveform_svg(report: &ReplayReport, title: &str, reconstructed: bool) -> String {
    let points = &report.waveform;
    let width = 1100.0;
    let height = 450.0;
    let left = 72.0;
    let right = 1076.0;
    let top = 54.0;
    let bottom = 388.0;
    let min_time = points.first().map_or(0, |point| point.time_ms);
    let max_time = points
        .last()
        .map_or(1, |point| point.time_ms)
        .max(min_time + 1);
    let x = |time_ms: u64| {
        left + (time_ms.saturating_sub(min_time) as f64 / (max_time - min_time) as f64)
            * (right - left)
    };
    let y = |luma: f64| bottom - luma.clamp(0.0, 1.0) * (bottom - top);
    let polyline = points
        .iter()
        .map(|point| format!("{:.2},{:.2}", x(point.time_ms), y(point.output_luma)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut svg = String::new();
    writeln!(
        svg,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" viewBox=\"0 0 {width} {height}\">"
    )
    .unwrap();
    writeln!(
        svg,
        "<rect width=\"100%\" height=\"100%\" fill=\"#101820\"/>"
    )
    .unwrap();
    writeln!(
        svg,
        "<text x=\"64\" y=\"26\" fill=\"#f4f1e8\" font-family=\"sans-serif\" font-size=\"17\" font-weight=\"700\">{}</text>",
        xml_escape(title)
    )
    .unwrap();
    writeln!(
        svg,
        "<text x=\"64\" y=\"43\" fill=\"#8fa4b3\" font-family=\"sans-serif\" font-size=\"11\">青色=present 橙色=compensated x=PTS/ms y=output luma{}</text>",
        if reconstructed {
            " (legacy envelope reconstruction)"
        } else {
            " (GStreamer GRAY8 measurement)"
        }
    )
    .unwrap();
    for tick in 0..=4 {
        let luma = tick as f64 / 4.0;
        let line_y = y(luma);
        writeln!(
            svg,
            "<line x1=\"{left:.2}\" y1=\"{line_y:.2}\" x2=\"{right:.2}\" y2=\"{line_y:.2}\" stroke=\"#263746\" stroke-width=\"1\"/>"
        )
        .unwrap();
        writeln!(
            svg,
            "<text x=\"56.00\" y=\"{:.2}\" fill=\"#8fa4b3\" font-family=\"monospace\" font-size=\"10\" text-anchor=\"end\">{luma:.2}</text>",
            line_y + 4.0
        )
        .unwrap();
    }
    writeln!(
        svg,
        "<polyline points=\"{polyline}\" fill=\"none\" stroke=\"#5bc0eb\" stroke-width=\"2.2\" stroke-linejoin=\"round\"/>"
    )
    .unwrap();
    for point in points {
        let color = if point.source_state == crate::model::FrameState::Missing {
            "#f4a261"
        } else {
            "#57d68d"
        };
        writeln!(
            svg,
            "<circle cx=\"{:.2}\" cy=\"{:.2}\" r=\"3.4\" fill=\"{color}\"><title>{}</title></circle>",
            x(point.time_ms),
            y(point.output_luma),
            xml_escape(&format!(
                "t={} ms pts={} ns track={}:{} frame={} luma={:.4} measured={:.4} params={} evidence={}",
                point.time_ms,
                point.pts_ns,
                point.track_id,
                point.track_index,
                point.frame,
                point.output_luma,
                point.measured_luma,
                point.params_fingerprint,
                point.evidence_sha256
            ))
        )
        .unwrap();
    }
    writeln!(
        svg,
        "<text x=\"1076\" y=\"420\" fill=\"#8fa4b3\" font-family=\"monospace\" font-size=\"10\" text-anchor=\"end\">first_error={} evidence_tip={}</text>",
        report
            .first_error
            .as_ref()
            .map_or("none".to_string(), |error| error.rule.clone()),
        report.evidence_tip()
    )
    .unwrap();
    svg.push_str("</svg>\n");
    svg
}

fn xml_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(feature = "gstreamer")]
pub struct GstLumaProcessor {
    pipeline: gstreamer::Pipeline,
    appsrc: gstreamer_app::AppSrc,
    appsink: gstreamer_app::AppSink,
    width: u32,
    height: u32,
}

#[cfg(feature = "gstreamer")]
impl GstLumaProcessor {
    pub fn new() -> Result<Self> {
        use gstreamer::prelude::*;
        gstreamer::init().context("initialize GStreamer")?;
        let pipeline = gstreamer::Pipeline::new();
        let appsrc = gstreamer::ElementFactory::make("appsrc")
            .name("review-frame-source")
            .build()
            .context("create appsrc")?
            .downcast::<gstreamer_app::AppSrc>()
            .map_err(|_| anyhow!("appsrc had an unexpected type"))?;
        let convert = gstreamer::ElementFactory::make("videoconvert")
            .name("review-luma-convert")
            .build()
            .context("create videoconvert")?;
        let sink_caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "GRAY8")
            .build();
        let appsink = gstreamer::ElementFactory::make("appsink")
            .name("review-waveform-sink")
            .property("sync", false)
            .property("max-buffers", 1u32)
            .property("drop", false)
            .property("caps", &sink_caps)
            .build()
            .context("create appsink")?
            .downcast::<gstreamer_app::AppSink>()
            .map_err(|_| anyhow!("appsink had an unexpected type"))?;
        pipeline
            .add_many([appsrc.upcast_ref(), &convert, appsink.upcast_ref()])
            .context("add review elements to pipeline")?;
        gstreamer::Element::link_many([appsrc.upcast_ref(), &convert, appsink.upcast_ref()])
            .context("link review pipeline")?;
        let width = 64;
        let height = 36;
        let source_caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "I420")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gstreamer::Fraction::new(30, 1))
            .build();
        appsrc.set_caps(Some(&source_caps));
        appsrc.set_format(gstreamer::Format::Time);
        pipeline
            .set_state(gstreamer::State::Playing)
            .context("start review pipeline")?;
        Ok(Self {
            pipeline,
            appsrc,
            appsink,
            width,
            height,
        })
    }
}

#[cfg(feature = "gstreamer")]
impl FrameProcessor for GstLumaProcessor {
    fn measure_luma(&mut self, point: &WavePoint) -> Result<f64> {
        let y_size = (self.width * self.height) as usize;
        let chroma_size = y_size / 4;
        let mut frame = vec![128u8; y_size + chroma_size * 2];
        let y_value = (point.output_luma.clamp(0.0, 1.0) * 255.0).round() as u8;
        frame[..y_size].fill(y_value);
        let mut buffer = gstreamer::Buffer::from_mut_slice(frame);
        let pts = u64::try_from(point.pts_ns).context("negative PTS cannot be replayed")?;
        buffer
            .get_mut()
            .context("frame buffer unexpectedly shared")?
            .set_pts(gstreamer::ClockTime::from_nseconds(pts));
        self.appsrc
            .push_buffer(buffer)
            .map_err(|failure| anyhow!("push frame to GStreamer: {failure:?}"))?;
        let sample = self
            .appsink
            .pull_sample()
            .context("GStreamer produced no output sample")?;
        let buffer = sample.buffer().context("output sample has no buffer")?;
        let map = buffer
            .map_readable()
            .context("map GStreamer output buffer")?;
        if map.as_slice().is_empty() {
            bail!("GStreamer returned an empty GRAY8 frame");
        }
        let sum: u64 = map.as_slice().iter().map(|value| u64::from(*value)).sum();
        Ok(sum as f64 / (map.as_slice().len() as f64 * 255.0))
    }
}

#[cfg(feature = "gstreamer")]
impl Drop for GstLumaProcessor {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gstreamer::State::Null);
    }
}
