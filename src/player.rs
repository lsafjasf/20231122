//! GStreamer playback of audition material via appsrc.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

pub struct Player {
    inner: Mutex<Option<Active>>,
}

struct Active {
    pipeline: gst::Pipeline,
    cancel: Arc<AtomicBool>,
    feeder: Option<std::thread::JoinHandle<()>>,
}

impl Player {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(None),
        })
    }

    /// Play `samples` (mono f32) `repeats` times, then stop.
    pub fn play(&self, samples: Arc<Vec<f32>>, sample_rate: u32, repeats: usize) {
        self.stop();
        let cancel = Arc::new(AtomicBool::new(false));
        let Ok(pipeline) = build_pipeline(sample_rate) else {
            return;
        };
        let appsrc = pipeline
            .by_name("src")
            .and_then(|e| e.dynamic_cast::<gst_app::AppSrc>().ok());
        let Some(appsrc) = appsrc else { return };
        if pipeline.set_state(gst::State::Playing).is_err() {
            return;
        }
        let cancel2 = cancel.clone();
        let feeder = std::thread::spawn(move || {
            feed(&appsrc, &samples, sample_rate, repeats, &cancel2);
        });
        *self.inner.lock().unwrap() = Some(Active {
            pipeline,
            cancel,
            feeder: Some(feeder),
        });
    }

    pub fn stop(&self) {
        let mut guard = self.inner.lock().unwrap();
        if let Some(mut active) = guard.take() {
            active.cancel.store(true, Ordering::Relaxed);
            let _ = active.pipeline.set_state(gst::State::Null);
            if let Some(h) = active.feeder.take() {
                let _ = h.join();
            }
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

fn build_pipeline(sample_rate: u32) -> Result<gst::Pipeline, ()> {
    let pipeline = gst::Pipeline::new();
    let src = gst_app::AppSrc::builder()
        .name("src")
        .caps(
            &gst::Caps::builder("audio/x-raw")
                .field("format", "F32LE")
                .field("rate", sample_rate as i32)
                .field("channels", 1i32)
                .field("layout", "interleaved")
                .build(),
        )
        .format(gst::Format::Time)
        .max_bytes(1 << 22)
        .build();
    let convert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(|_| ())?;
    let sink = gst::ElementFactory::make("autoaudiosink")
        .build()
        .map_err(|_| ())?;
    pipeline
        .add_many([src.upcast_ref(), &convert, &sink])
        .map_err(|_| ())?;
    gst::Element::link_many([src.upcast_ref(), &convert, &sink]).map_err(|_| ())?;
    Ok(pipeline)
}

fn feed(
    appsrc: &gst_app::AppSrc,
    samples: &[f32],
    sample_rate: u32,
    repeats: usize,
    cancel: &AtomicBool,
) {
    const CHUNK: usize = 8192;
    let mut pts = gst::ClockTime::ZERO;
    let frame_dur = gst::ClockTime::SECOND / sample_rate as u64;
    for _ in 0..repeats {
        for chunk in samples.chunks(CHUNK) {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let mut buffer = gst::Buffer::with_size(chunk.len() * 4).unwrap();
            {
                let buf = buffer.get_mut().unwrap();
                buf.set_pts(pts);
                buf.set_duration(frame_dur * chunk.len() as u64);
                let mut map = buf.map_writable().unwrap();
                for (i, &s) in chunk.iter().enumerate() {
                    map.as_mut()[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
                }
            }
            pts += frame_dur * chunk.len() as u64;
            if appsrc.push_buffer(buffer).is_err() {
                return;
            }
        }
    }
    let _ = appsrc.end_of_stream();
}

#[cfg(test)]
mod tests {
    #[test]
    fn pipeline_builds() {
        gstreamer::init().unwrap();
        assert!(super::build_pipeline(44100).is_ok());
    }
}
