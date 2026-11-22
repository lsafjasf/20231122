use std::path::Path;

use anyhow::{Context, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::{AppSink, AppSrc};
use gstreamer_video::VideoInfo;

pub const MAX_FRAMES: u64 = 1_000_000;

pub struct FrameView {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub pts_ns: u64,
    pub duration_ns: u64,
}

pub fn read_video<F>(path: &Path, mut on_frame: F) -> anyhow::Result<(u32, u32)>
where
    F: FnMut(FrameView) -> anyhow::Result<()>,
{
    let source = gst::ElementFactory::make("filesrc")
        .property("location", path.to_string_lossy().as_ref())
        .build()?;
    let demux = gst::ElementFactory::make("decodebin").build()?;
    let convert = gst::ElementFactory::make("videoconvert").build()?;
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .build();
    let capsfilter = gst::ElementFactory::make("capsfilter")
        .property("caps", &caps)
        .build()?;
    let sink_element = gst::ElementFactory::make("appsink")
        .property("sync", false)
        .property("emit-signals", false)
        .build()?;
    let sink = sink_element
        .clone()
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("appsink downcast failed"))?;
    sink.set_wait_on_eos(true);

    let pipeline = gst::Pipeline::new();
    pipeline.add_many([&source, &demux, &convert, &capsfilter, &sink_element])?;
    gst::Element::link_many([&source, &demux])?;
    gst::Element::link_many([&convert, &capsfilter, &sink_element])?;
    let convert_weak = convert.downgrade();
    demux.connect_pad_added(move |_, pad| {
        let Some(convert) = convert_weak.upgrade() else {
            return;
        };
        let caps = pad.current_caps().or_else(|| Some(pad.query_caps(None)));
        let is_video = caps
            .as_ref()
            .and_then(|caps| caps.structure(0))
            .is_some_and(|structure| structure.name().starts_with("video/"));
        if !is_video {
            return;
        }
        if let Some(sink) = convert.static_pad("sink") {
            let _ = pad.link(&sink);
        }
    });
    pipeline.set_state(gst::State::Playing)?;

    let mut dimensions = None;
    let mut frame_count = 0u64;
    let result = loop {
        let Some(sample) = sink.try_pull_sample(gst::ClockTime::from_seconds(10)) else {
            break check_bus(&pipeline);
        };
        let caps = sample.caps().context("decoded sample has no caps")?;
        let info = VideoInfo::from_caps(caps)?;
        let width = info.width();
        let height = info.height();
        let stride = *info.stride().first().context("missing RGBA stride")? as usize;
        let buffer = sample.buffer().context("decoded sample has no buffer")?;
        let map = buffer.map_readable()?;
        let row_bytes = width as usize * 4;
        if stride < row_bytes {
            bail!("invalid RGBA stride {stride} for width {width}");
        }
        let mut rgba = vec![0u8; row_bytes * height as usize];
        for row in 0..height as usize {
            let source_start = row * stride;
            let destination_start = row * row_bytes;
            let source_end = source_start + row_bytes;
            if source_end > map.len() {
                bail!("decoded frame row {row} exceeds mapped buffer");
            }
            rgba[destination_start..destination_start + row_bytes]
                .copy_from_slice(&map[source_start..source_end]);
        }
        let pts_ns = buffer.pts().map_or(0, |time| time.nseconds());
        let duration_ns = buffer.duration().map_or(0, |time| time.nseconds());
        frame_count += 1;
        if frame_count > MAX_FRAMES {
            break Err(anyhow::anyhow!(
                "video exceeds maximum supported frame count {MAX_FRAMES}"
            ));
        }
        dimensions = Some((width, height));
        if let Err(error) = on_frame(FrameView {
            rgba,
            width,
            height,
            pts_ns,
            duration_ns,
        }) {
            break Err(error);
        }
    };

    let _ = pipeline.set_state(gst::State::Null);
    result?;
    dimensions.context("video contains no decodable frames")
}

fn check_bus(pipeline: &gst::Pipeline) -> anyhow::Result<()> {
    let Some(bus) = pipeline.bus() else {
        return Ok(());
    };
    while let Some(message) = bus.pop() {
        use gst::MessageView;
        match message.view() {
            MessageView::Eos(..) => return Ok(()),
            MessageView::Error(error) => {
                bail!(
                    "GStreamer input error from {}: {} ({:?})",
                    error
                        .src()
                        .map(|source| source.path_string().to_string())
                        .unwrap_or_else(|| "unknown".into()),
                    error.error(),
                    error.debug()
                );
            }
            _ => {}
        }
    }
    Ok(())
}

pub struct OutputWriter {
    pipeline: gst::Pipeline,
    source: AppSrc,
    width: u32,
    height: u32,
}

impl OutputWriter {
    pub fn new(path: &Path, width: u32, height: u32) -> anyhow::Result<Self> {
        let source_element = gst::ElementFactory::make("appsrc")
            .property("format", gst::Format::Time)
            .property("is-live", false)
            .build()?;
        let source = source_element
            .clone()
            .downcast::<AppSrc>()
            .map_err(|_| anyhow::anyhow!("appsrc downcast failed"))?;
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGBA")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gst::Fraction::new(10, 1))
            .build();
        source.set_caps(Some(&caps));
        source.set_format(gst::Format::Time);

        let convert = gst::ElementFactory::make("videoconvert").build()?;
        let rgb_caps = gst::Caps::builder("video/x-raw")
            .field("format", "RGB")
            .build();
        let capsfilter = gst::ElementFactory::make("capsfilter")
            .property("caps", &rgb_caps)
            .build()?;
        let mux = gst::ElementFactory::make("matroskamux").build()?;
        let sink = gst::ElementFactory::make("filesink")
            .property("location", path.to_string_lossy().as_ref())
            .build()?;
        let pipeline = gst::Pipeline::new();
        pipeline.add_many([&source_element, &convert, &capsfilter, &mux, &sink])?;
        gst::Element::link_many([&source_element, &convert, &capsfilter, &mux, &sink])?;
        pipeline.set_state(gst::State::Playing)?;
        Ok(Self {
            pipeline,
            source,
            width,
            height,
        })
    }

    pub fn push(&self, rgba: &[u8], pts_ns: u64, duration_ns: u64) -> anyhow::Result<()> {
        let expected = self.width as usize * self.height as usize * 4;
        if rgba.len() != expected {
            bail!(
                "frame has {} bytes, expected {expected} for {}x{}",
                rgba.len(),
                self.width,
                self.height
            );
        }
        let mut buffer = gst::Buffer::from_mut_slice(rgba.to_vec());
        {
            let buffer = buffer.get_mut().context("new buffer is not writable")?;
            buffer.set_pts(gst::ClockTime::from_nseconds(pts_ns));
            if duration_ns > 0 {
                buffer.set_duration(gst::ClockTime::from_nseconds(duration_ns));
            }
        }
        self.source.push_buffer(buffer)?;
        Ok(())
    }

    pub fn finish(self) -> anyhow::Result<()> {
        self.source.end_of_stream()?;
        let bus = self.pipeline.bus().context("output pipeline has no bus")?;
        let timeout = gst::ClockTime::from_seconds(10);
        while let Some(message) = bus.timed_pop(timeout) {
            use gst::MessageView;
            match message.view() {
                MessageView::Eos(..) => break,
                MessageView::Error(error) => {
                    bail!(
                        "GStreamer output error: {} ({:?})",
                        error.error(),
                        error.debug()
                    );
                }
                _ => {}
            }
        }
        self.pipeline.set_state(gst::State::Null)?;
        Ok(())
    }
}

pub fn draw_trail(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    trail: &[(f64, f64)],
    anchor: Option<&str>,
) {
    let progress = trail.len() as u32;
    for x in 0..width {
        let lit = x <= progress.saturating_mul(width) / 40;
        let color = if lit {
            [80, 220, 200, 190]
        } else {
            [40, 50, 62, 160]
        };
        set_pixel(rgba, width, height, x as i32, 1, color);
    }
    for window in trail.windows(2) {
        draw_line(
            rgba,
            width,
            height,
            window[0],
            window[1],
            [62, 214, 198, 230],
            2,
        );
    }
    for (index, point) in trail.iter().enumerate() {
        let alpha = if index + 1 == trail.len() { 255 } else { 150 };
        draw_circle(
            rgba,
            width,
            height,
            *point,
            if index + 1 == trail.len() { 4 } else { 2 },
            [238, 246, 252, alpha],
        );
    }
    if let Some(anchor) = anchor {
        let color = match anchor {
            "A" => [122, 162, 247, 255],
            "B" => [192, 153, 255, 255],
            "C" => [247, 118, 142, 255],
            _ => [255, 255, 255, 255],
        };
        if let Some(point) = trail.last() {
            draw_circle(rgba, width, height, *point, 9, color);
            let ring = [(point.0 - 13.0, point.1), (point.0 + 13.0, point.1)];
            draw_line(rgba, width, height, ring[0], ring[1], color, 1);
            let vertical = [(point.0, point.1 - 13.0), (point.0, point.1 + 13.0)];
            draw_line(rgba, width, height, vertical[0], vertical[1], color, 1);
        }
    }
}

fn draw_line(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    start: (f64, f64),
    end: (f64, f64),
    color: [u8; 4],
    thickness: i32,
) {
    let x0 = start.0.round() as i32;
    let y0 = start.1.round() as i32;
    let x1 = end.0.round() as i32;
    let y1 = end.1.round() as i32;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        draw_circle(rgba, width, height, (x as f64, y as f64), thickness, color);
        if x == x1 && y == y1 {
            break;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x += sx;
        }
        if doubled <= dx {
            error += dx;
            y += sy;
        }
    }
}

fn draw_circle(
    rgba: &mut [u8],
    width: u32,
    height: u32,
    center: (f64, f64),
    radius: i32,
    color: [u8; 4],
) {
    let cx = center.0.round() as i32;
    let cy = center.1.round() as i32;
    for y in cy - radius..=cy + radius {
        for x in cx - radius..=cx + radius {
            if (x - cx).pow(2) + (y - cy).pow(2) <= radius.pow(2) {
                set_pixel(rgba, width, height, x, y, color);
            }
        }
    }
}

fn set_pixel(rgba: &mut [u8], width: u32, height: u32, x: i32, y: i32, color: [u8; 4]) {
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let offset = ((y as u32 * width + x as u32) * 4) as usize;
    let alpha = color[3] as u32;
    for channel in 0..3 {
        let source = color[channel] as u32;
        let destination = rgba[offset + channel] as u32;
        rgba[offset + channel] = ((source * alpha + destination * (255 - alpha)) / 255) as u8;
    }
    rgba[offset + 3] = 255;
}

pub fn write_png(rgba: &[u8], width: u32, height: u32, path: &Path) -> anyhow::Result<()> {
    let source_element = gst::ElementFactory::make("appsrc")
        .property("format", gst::Format::Time)
        .property("is-live", false)
        .build()?;
    let source = source_element
        .clone()
        .downcast::<AppSrc>()
        .map_err(|_| anyhow::anyhow!("snapshot appsrc downcast failed"))?;
    let caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field("width", width as i32)
        .field("height", height as i32)
        .field("framerate", gst::Fraction::new(10, 1))
        .build();
    source.set_caps(Some(&caps));
    let convert = gst::ElementFactory::make("videoconvert").build()?;
    let encoder = gst::ElementFactory::make("pngenc").build()?;
    let sink = gst::ElementFactory::make("filesink")
        .property("location", path.to_string_lossy().as_ref())
        .build()?;
    let pipeline = gst::Pipeline::new();
    pipeline.add_many([&source_element, &convert, &encoder, &sink])?;
    gst::Element::link_many([&source_element, &convert, &encoder, &sink])?;
    pipeline.set_state(gst::State::Playing)?;
    source.push_buffer(gst::Buffer::from_mut_slice(rgba.to_vec()))?;
    source.end_of_stream()?;
    let bus = pipeline.bus().context("snapshot pipeline has no bus")?;
    while let Some(message) = bus.timed_pop(gst::ClockTime::from_seconds(5)) {
        use gst::MessageView;
        match message.view() {
            MessageView::Eos(..) => break,
            MessageView::Error(error) => bail!("PNG snapshot error: {}", error.error()),
            _ => {}
        }
    }
    pipeline.set_state(gst::State::Null)?;
    Ok(())
}
