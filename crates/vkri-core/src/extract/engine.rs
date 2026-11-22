//! GStreamer implementation of the extraction engine.
//!
//! Separate from `mod.rs` so policy/types stay readable and the GStreamer
//! plumbing can be reviewed as one unit.

#[cfg(feature = "gstreamer")]
use {
    crate::audit::AuditLog,
    crate::extract::*,
    crate::media,
    crate::storage::ProjectStore,
    crate::{Error, Result},
    gstreamer::prelude::*,
    gstreamer::{Buffer, ClockTime, Element, ElementFactory, Pipeline},
    gstreamer_app::AppSink,
    std::fs,
};

#[cfg(feature = "gstreamer")]
fn pipeline_error(error: glib::BoolError) -> Error {
    Error::Pipeline(error.to_string())
}

#[cfg(feature = "gstreamer")]
fn make_element(factory: &str) -> Result<Element> {
    ElementFactory::make(factory)
        .build()
        .map_err(pipeline_error)
}

#[cfg(feature = "gstreamer")]
pub fn extract_segment(
    store: &ProjectStore,
    segment: &mut MediaSegment,
    offset: crate::time::ClockOffset,
    actor: &str,
    cancel: &CancellationToken,
    progress: impl Fn(Progress),
) -> Result<ExtractionOutcome> {
    media::init_gstreamer()?;
    media::verify_source(&segment.source_path, &segment.source_sha256)?;

    let run_id = new_id("run");
    let started_at = crate::time::format_unix_nanos(
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
    );
    let paths = run_paths(&store.evidence_root(), &segment.id, &run_id);
    fs::create_dir_all(&paths.run_dir).map_err(|e| Error::io(&paths.run_dir, e))?;

    let mut audit = store.audit_for(&segment.id)?;
    audit.append(
        actor,
        "run.started",
        &format!(
            "mode={:?} interval_ns={}",
            segment.config.mode, segment.config.interval_ns
        ),
        Some(&run_id),
    )?;

    let result = run_pipeline(
        &paths, segment, offset, &run_id, cancel, &progress, &mut audit,
    );

    let summary = match result {
        Ok(summary) => summary,
        Err(error) => {
            audit
                .append(actor, "run.failed", &error.to_string(), Some(&run_id))
                .ok();
            return Err(error);
        }
    };
    let PipelineSummary {
        keyframes,
        frames_seen,
        last_pts,
        interrupted,
        message,
    } = summary;

    let finished_at = crate::time::format_unix_nanos(
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default(),
    );
    let record = RunRecord {
        run_id: run_id.clone(),
        started_at,
        finished_at: Some(finished_at.clone()),
        completed: !interrupted,
        cancelled: interrupted,
        config: segment.config.clone(),
        frames_seen,
        keyframes_written: keyframes.len() as u64,
        last_pts_ns: last_pts,
        message: message.clone(),
    };

    let manifest = RunManifest {
        run_id: run_id.clone(),
        segment_id: segment.id.clone(),
        source_sha256: segment.source_sha256.clone(),
        config: segment.config.clone(),
        started_at: record.started_at.clone(),
        finished_at: Some(finished_at),
        completed: !interrupted,
        cancelled: interrupted,
        frames_seen,
        keyframes: keyframes.clone(),
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    fs::write(&paths.manifest_path, manifest_bytes)
        .map_err(|e| Error::io(&paths.manifest_path, e))?;

    if !interrupted {
        // The previous keyframe set is replaced only after a fully successful
        // run; older run directories stay on disk as retained evidence.
        segment.keyframes = keyframes.clone();
        segment.status = SegmentStatus::Extracted;
        segment.last_error = None;
        audit.append(
            actor,
            "run.completed",
            &format!("keyframes={}", keyframes.len()),
            Some(&run_id),
        )?;
    } else {
        segment.status = SegmentStatus::Interrupted;
        segment.last_error = Some(message.clone());
        audit.append(
            actor,
            "run.interrupted",
            &format!(
                "keyframes_partial={} frames_seen={frames_seen}",
                keyframes.len()
            ),
            Some(&run_id),
        )?;
    }
    segment.runs.push(record.clone());

    Ok(ExtractionOutcome {
        run_id,
        record,
        keyframes,
        interrupted,
    })
}

#[cfg(feature = "gstreamer")]
struct PipelineSummary {
    keyframes: Vec<Keyframe>,
    frames_seen: u64,
    last_pts: Option<i64>,
    interrupted: bool,
    message: String,
}

/// Reusable JPEG encoder: RGB frames in, JPEG bytes out.
#[cfg(feature = "gstreamer")]
struct JpegEncoder {
    pipeline: Pipeline,
    source: gstreamer_app::AppSrc,
    sink: AppSink,
}

#[cfg(feature = "gstreamer")]
impl JpegEncoder {
    fn new(max_edge: u32, quality: u32) -> Result<Self> {
        let _ = max_edge;
        let pipeline = Pipeline::new();
        let source = gstreamer_app::AppSrc::builder()
            .name("vkri-src")
            .block(true)
            .format(gstreamer::Format::Time)
            .build();
        let jpegenc = make_element("jpegenc")?;
        jpegenc.set_property("quality", quality.clamp(1, 100) as i32);
        let sink = AppSink::builder()
            .name("vkri-jpeg-sink")
            .sync(false)
            .max_buffers(1)
            .drop(false)
            .build();
        let sink_element = sink.upcast_ref::<Element>().clone();

        pipeline
            .add_many([source.upcast_ref::<Element>(), &jpegenc, &sink_element])
            .map_err(pipeline_error)?;
        Element::link_many([source.upcast_ref::<Element>(), &jpegenc, &sink_element])
            .map_err(pipeline_error)?;
        pipeline
            .set_state(gstreamer::State::Playing)
            .map_err(|e| Error::Pipeline(format!("jpeg pipeline start: {e:?}")))?;
        Ok(Self {
            pipeline,
            source,
            sink,
        })
    }

    /// Encode one RGB frame. `width`/`height` describe the (already scaled)
    /// frame dimensions.
    fn encode_rgb(&self, width: u32, height: u32, pts: ClockTime, rgb: &[u8]) -> Result<Vec<u8>> {
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "RGB")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gstreamer::Fraction::new(1, 1))
            .build();
        let mut buffer = Buffer::from_mut_slice(rgb.to_vec());
        buffer
            .get_mut()
            .ok_or_else(|| Error::Pipeline("fresh buffer unexpectedly shared".into()))?
            .set_pts(pts);
        let sample = gstreamer::Sample::builder()
            .caps(&caps)
            .buffer(&buffer)
            .build();
        self.source
            .push_sample(&sample)
            .map_err(|e| Error::Pipeline(format!("appsrc push: {e:?}")))?;
        let encoded = self
            .sink
            .pull_sample()
            .map_err(|e| Error::Pipeline(format!("appsink pull jpeg: {e:?}")))?;
        let encoded_buffer = encoded
            .buffer()
            .ok_or_else(|| Error::Pipeline("jpeg sample without buffer".into()))?;
        let map = encoded_buffer
            .map_readable()
            .map_err(|e| Error::Pipeline(format!("jpeg map: {e:?}")))?;
        Ok(map.as_slice().to_vec())
    }
}

#[cfg(feature = "gstreamer")]
impl Drop for JpegEncoder {
    fn drop(&mut self) {
        let _ = self.source.end_of_stream();
        let _ = self.pipeline.set_state(gstreamer::State::Null);
    }
}

#[cfg(feature = "gstreamer")]
#[allow(clippy::too_many_arguments)]
fn run_pipeline(
    paths: &RunPaths,
    segment: &MediaSegment,
    offset: crate::time::ClockOffset,
    _run_id: &str,
    cancel: &CancellationToken,
    progress: &dyn Fn(Progress),
    audit: &mut AuditLog,
) -> Result<PipelineSummary> {
    let pipeline = Pipeline::new();
    let filesrc = make_element("filesrc")?;
    filesrc.set_property("location", &segment.source_path);
    let decodebin = make_element("decodebin3")?;
    let queue = make_element("queue")?;
    queue.set_property("max-size-buffers", 0u32);
    let convert = make_element("videoconvertscale")?;
    let appsink = AppSink::builder()
        .name("vkri-sink")
        .sync(false)
        .max_buffers(8)
        .drop(false)
        .build();
    let appsink_element = appsink.upcast_ref::<Element>().clone();
    let rgb_caps = gstreamer::Caps::builder("video/x-raw")
        .field("format", "RGB")
        .build();
    appsink.set_caps(Some(&rgb_caps));

    pipeline
        .add_many([&filesrc, &decodebin, &queue, &convert, &appsink_element])
        .map_err(pipeline_error)?;
    Element::link(&filesrc, &decodebin).map_err(pipeline_error)?;
    Element::link_many([&queue, &convert, &appsink_element]).map_err(pipeline_error)?;

    let queue_weak = queue.downgrade();
    decodebin.connect_pad_added(move |_bin, pad| {
        let is_non_video = pad
            .current_caps()
            .and_then(|caps| caps.structure(0).map(|s| s.name().starts_with("audio/")))
            .unwrap_or(false)
            || pad
                .current_caps()
                .and_then(|caps| {
                    caps.structure(0)
                        .map(|s| s.name().starts_with("audio/") || s.name().starts_with("text/"))
                })
                .unwrap_or(false);
        // decodebin3 may emit pad-added before caps are negotiated; only skip
        // pads positively identified as non-video, otherwise try to link.
        if is_non_video {
            return;
        }
        if let Some(queue) = queue_weak.upgrade() {
            if let Some(sink_pad) = queue.static_pad("sink") {
                if !sink_pad.is_linked() {
                    if let Err(link_error) = pad.link(&sink_pad) {
                        gstreamer::warning!(
                            gstreamer::CAT_DEFAULT,
                            "decodebin pad link failed: {link_error:?}"
                        );
                    }
                }
            }
        }
    });

    let bus = pipeline
        .bus()
        .ok_or_else(|| Error::Pipeline("no bus".into()))?;
    pipeline
        .set_state(gstreamer::State::Playing)
        .map_err(|e| Error::Pipeline(format!("pipeline start: {e:?}")))?;

    let mut selector = FrameSelector::new(&segment.config);
    let encoder = JpegEncoder::new(
        segment.config.thumbnail_max_edge,
        segment.config.jpeg_quality,
    )?;
    let mut keyframes = Vec::new();
    let mut frames_seen = 0u64;
    let mut last_pts = None;
    let mut interrupted = false;

    'outer: loop {
        if cancel.is_cancelled() {
            interrupted = true;
            audit
                .append(
                    "system",
                    "run.cancel_signalled",
                    &format!("frames_seen={frames_seen}"),
                    None,
                )
                .ok();
            break;
        }

        if let Some(message) =
            bus.pop_filtered(&[gstreamer::MessageType::Error, gstreamer::MessageType::Eos])
        {
            match message.view() {
                gstreamer::MessageView::Error(error) => {
                    let _ = pipeline.set_state(gstreamer::State::Null);
                    let gerror = error.error();
                    let debug = error.debug();
                    return Err(Error::Pipeline(format!(
                        "{gerror} ({})",
                        debug.unwrap_or_default()
                    )));
                }
                gstreamer::MessageView::Eos(_) => break 'outer,
                _ => {}
            }
        }

        let sample = match appsink.try_pull_sample(ClockTime::from_mseconds(250)) {
            Some(sample) => sample,
            None => {
                if appsink.is_eos() {
                    break;
                }
                continue;
            }
        };
        let buffer = match sample.buffer() {
            Some(buffer) => buffer,
            None => continue,
        };
        let pts = buffer.pts().ok_or_else(|| {
            Error::Pipeline("decoded frame without presentation timestamp".into())
        })?;
        let pts_ns = pts.nseconds() as i64;
        frames_seen += 1;
        last_pts = Some(pts_ns);

        let is_decoder_keyframe = !buffer.flags().contains(gstreamer::BufferFlags::DELTA_UNIT);
        if !selector.should_keep(pts_ns, is_decoder_keyframe) {
            progress(Progress {
                frames_seen,
                keyframes_written: keyframes.len() as u64,
                pts_ns,
            });
            continue;
        }

        let (scaled_width, scaled_height, rgb) =
            extract_rgb_scaled(&sample, segment.config.thumbnail_max_edge)?;
        let jpeg_bytes = encoder.encode_rgb(scaled_width, scaled_height, pts, &rgb)?;
        let index = keyframes.len() as u32;
        let thumbnail_file = format!("kf-{:06}.jpg", index + 1);
        let thumbnail_path = paths.run_dir.join(&thumbnail_file);
        fs::write(&thumbnail_path, &jpeg_bytes).map_err(|e| Error::io(&thumbnail_path, e))?;
        let thumbnail_sha256 = crate::hash::sha256_bytes(&jpeg_bytes);
        let keyframe = keyframe_from_buffer(
            index,
            pts_ns,
            is_decoder_keyframe,
            thumbnail_sha256,
            thumbnail_file,
            jpeg_bytes.len() as u64,
            segment,
            offset,
        );
        selector.note_kept(pts_ns);
        keyframes.push(keyframe);

        progress(Progress {
            frames_seen,
            keyframes_written: keyframes.len() as u64,
            pts_ns,
        });
    }

    let _ = pipeline.set_state(gstreamer::State::Null);
    let message = if interrupted {
        "cancelled before end of stream; partial evidence retained".to_string()
    } else {
        "end of stream reached".to_string()
    };
    Ok(PipelineSummary {
        keyframes,
        frames_seen,
        last_pts,
        interrupted,
        message,
    })
}

/// Map an appsink sample to scaled RGB8 bytes for the JPEG encoder.
#[cfg(feature = "gstreamer")]
fn extract_rgb_scaled(sample: &gstreamer::Sample, max_edge: u32) -> Result<(u32, u32, Vec<u8>)> {
    let caps = sample
        .caps()
        .ok_or_else(|| Error::Pipeline("sample without caps".into()))?;
    let structure = caps
        .structure(0)
        .ok_or_else(|| Error::Pipeline("caps without structure".into()))?;
    let width: i32 = structure
        .get("width")
        .map_err(|e| Error::Pipeline(e.to_string()))?;
    let width = width as u32;
    let height = structure
        .get::<i32>("height")
        .map_err(|e| Error::Pipeline(e.to_string()))? as u32;
    let buffer = sample
        .buffer()
        .ok_or_else(|| Error::Pipeline("sample without buffer".into()))?;
    let map = buffer
        .map_readable()
        .map_err(|e| Error::Pipeline(format!("frame map: {e:?}")))?;

    // The videoconvertscale element is configured with RGB caps below; frames
    // arriving here are tightly packed RGB rows.
    let (scaled_width, scaled_height) = scaled_dimensions(width, height, max_edge);
    let rgb = if (scaled_width, scaled_height) == (width, height) {
        map.as_slice().to_vec()
    } else {
        resize_rgb_nearest(map.as_slice(), width, height, scaled_width, scaled_height)
    };
    Ok((scaled_width, scaled_height, rgb))
}

#[cfg(feature = "gstreamer")]
fn scaled_dimensions(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let max_edge = max_edge.max(1);
    let longest = width.max(height);
    if longest <= max_edge {
        return (width, height);
    }
    let scale = max_edge as f64 / longest as f64;
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

/// Small deterministic nearest-neighbour scaler for packed RGB8 frames.
#[cfg(feature = "gstreamer")]
fn resize_rgb_nearest(
    src: &[u8],
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Vec<u8> {
    let mut dst = vec![0u8; (dst_width * dst_height * 3) as usize];
    for y in 0..dst_height {
        let source_y =
            ((y as f64 * src_height as f64 / dst_height as f64).floor() as u32).min(src_height - 1);
        for x in 0..dst_width {
            let source_x = ((x as f64 * src_width as f64 / dst_width as f64).floor() as u32)
                .min(src_width - 1);
            let src_index = ((source_y * src_width + source_x) * 3) as usize;
            let dst_index = ((y * dst_width + x) * 3) as usize;
            dst[dst_index..dst_index + 3].copy_from_slice(&src[src_index..src_index + 3]);
        }
    }
    dst
}

#[cfg(not(feature = "gstreamer"))]
pub fn extract_segment(
    _store: &ProjectStore,
    _segment: &mut MediaSegment,
    _offset: crate::time::ClockOffset,
    _actor: &str,
    _cancel: &CancellationToken,
    _progress: impl Fn(Progress),
) -> Result<ExtractionOutcome> {
    Err(Error::Pipeline(
        "vkri-core built without the gstreamer feature".into(),
    ))
}
