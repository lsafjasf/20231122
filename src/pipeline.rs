use crate::metadata::FrameAnalysis;
use crate::model::CameraProfile;
use anyhow::{Context, anyhow};
use gstreamer as gst;
use gstreamer::glib::object::Cast;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc};
use std::sync::Once;

static GST_INIT: Once = Once::new();

#[derive(Debug, Clone)]
pub struct ProcessedFrame {
    pub analysis: FrameAnalysis,
    pub preview_rgba: Option<Vec<u8>>,
}

pub struct FramePipeline {
    pipeline: gst::Pipeline,
    appsrc: AppSrc,
    appsink: AppSink,
    width: u32,
    height: u32,
    frame_pts_ns: u64,
}

impl FramePipeline {
    pub fn new(profile: &CameraProfile) -> anyhow::Result<Self> {
        GST_INIT.call_once(|| {
            let _ = gst::init();
        });
        let pipeline = gst::Pipeline::new();
        let appsrc = gst::ElementFactory::make("appsrc")
            .name("camera-frames")
            .build()
            .context("create appsrc")?;
        let convert = gst::ElementFactory::make("videoconvert")
            .build()
            .context("create videoconvert")?;
        let appsink = gst::ElementFactory::make("appsink")
            .name("analysis-sink")
            .build()
            .context("create appsink")?;
        pipeline.add_many([&appsrc, &convert, &appsink])?;
        gst::Element::link_many([&appsrc, &convert, &appsink])?;

        let appsrc = appsrc
            .downcast::<AppSrc>()
            .map_err(|_| anyhow!("not appsrc"))?;
        let appsink = appsink
            .downcast::<AppSink>()
            .map_err(|_| anyhow!("not appsink"))?;
        appsrc.set_property("format", gst::Format::Time);
        appsrc.set_property("is-live", false);
        appsrc.set_property("block", true);
        appsink.set_property("sync", false);
        appsink.set_property("max-buffers", 1u32);
        appsink.set_property("drop", false);

        let input_caps = gst::Caps::builder("video/x-raw")
            .field("format", "GRAY16_LE")
            .field("width", profile.width as i32)
            .field("height", profile.height as i32)
            .field(
                "framerate",
                gst::Fraction::new(profile.fps_num as i32, profile.fps_den as i32),
            )
            .build();
        let output_caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .field("width", profile.width as i32)
            .field("height", profile.height as i32)
            .build();
        appsrc.set_property("caps", &input_caps);
        appsink.set_property("caps", &output_caps);
        pipeline.set_state(gst::State::Playing)?;

        Ok(Self {
            pipeline,
            appsrc,
            appsink,
            width: profile.width,
            height: profile.height,
            frame_pts_ns: profile
                .fps_den
                .saturating_mul(1_000_000_000)
                .checked_div(profile.fps_num)
                .unwrap_or(0) as u64,
        })
    }

    pub fn process(
        &self,
        gray16: Vec<u8>,
        pts_ns: u64,
        source_offset: u64,
        profile: &CameraProfile,
        pixel_step: u32,
        capture_preview: bool,
    ) -> anyhow::Result<ProcessedFrame> {
        anyhow::ensure!(
            gray16.len() as u64 == profile.frame_bytes(),
            "frame buffer size mismatch"
        );
        let mut buffer = gst::Buffer::from_mut_slice(gray16);
        {
            let buffer = buffer.get_mut().expect("fresh buffer");
            buffer.set_pts(gst::ClockTime::from_nseconds(pts_ns));
            buffer.set_duration(gst::ClockTime::from_nseconds(self.frame_pts_ns));
            buffer.set_offset(source_offset);
        }
        self.appsrc.push_buffer(buffer)?;
        let sample = self.appsink.pull_sample()?;
        let output = sample
            .buffer()
            .ok_or_else(|| anyhow!("GStreamer sample has no buffer"))?;
        let map = output
            .map_readable()
            .context("map GStreamer output buffer")?;
        let analysis = analyze_rgba(
            &map,
            self.width,
            self.height,
            pixel_step,
            &profile.repeated_object,
        );
        let preview_rgba = capture_preview.then(|| map.as_slice().to_vec());
        Ok(ProcessedFrame {
            analysis,
            preview_rgba,
        })
    }
}

impl Drop for FramePipeline {
    fn drop(&mut self) {
        let _ = self.appsrc.end_of_stream();
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

pub fn analyze_rgba(
    rgba: &[u8],
    width: u32,
    height: u32,
    pixel_step: u32,
    repeated: &crate::model::RepeatedObject,
) -> FrameAnalysis {
    let step = pixel_step.max(1) as usize;
    let width = width as usize;
    let height = height as usize;
    let mut peak = 0u16;
    let mut sum = 0u64;
    let mut count = 0u64;
    let mut object_present = false;
    let x0 = repeated.rect.x as usize;
    let y0 = repeated.rect.y as usize;
    let x1 = (repeated.rect.x + repeated.rect.width) as usize;
    let y1 = (repeated.rect.y + repeated.rect.height) as usize;
    for y in (0..height).step_by(step) {
        for x in (0..width).step_by(step) {
            let offset = (y * width + x) * 4;
            let r = u16::from(rgba[offset]);
            let g = u16::from(rgba[offset + 1]);
            let b = u16::from(rgba[offset + 2]);
            let luma = (r * 54 + g * 183 + b * 19) >> 8;
            let nits = ((u32::from(luma) * 10_000 + 127) / 255) as u16;
            peak = peak.max(nits);
            sum += u64::from(nits);
            count += 1;
            if x >= x0 && x < x1 && y >= y0 && y < y1 && nits >= repeated.threshold_nits {
                object_present = true;
            }
        }
    }
    let avg = (sum + count / 2).checked_div(count).unwrap_or(0) as u16;
    FrameAnalysis {
        peak_nits: peak,
        avg_nits: avg,
        object_present,
        sampled_pixels: count,
        total_pixels: (width * height) as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::demo_profile;

    #[test]
    fn analysis_detects_extreme_repeated_object() {
        let profile = demo_profile();
        let gray = crate::fixture::render_frame(72);
        let rgba = gray
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|px| {
                let value = u16::from_le_bytes([px[0], px[1]]);
                let byte = (value >> 8) as u8;
                [byte, byte, byte, 255]
            })
            .collect::<Vec<_>>();
        let result = analyze_rgba(
            &rgba,
            profile.width,
            profile.height,
            1,
            &profile.repeated_object,
        );
        assert!(result.object_present);
        assert!(result.peak_nits >= 8_000);
    }
}
