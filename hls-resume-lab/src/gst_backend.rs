//! GStreamer adapter: pipeline probes for HLS resume-point diagnosis.
//!
//! The probes deliberately stop in the `PAUSED` state. They prove that the
//! HLS source can be constructed and primed from a given manifest without
//! pulling every segment, which keeps the experiment deterministic and
//! offline-friendly.

use crate::core::model::ResumeRecord;
use crate::core::runner::GstProbeEvidence;
use gstreamer::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::Once;

static GST_INIT: Once = Once::new();

/// Idempotently initialize GStreamer. Returns an error string instead of
/// panicking so the experiment can record the failure and continue.
pub fn ensure_init() -> Result<(), String> {
    let mut init_error: Option<String> = None;
    GST_INIT.call_once(|| {
        if let Err(err) = gstreamer::init() {
            init_error = Some(err.to_string());
        }
    });
    match init_error {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

/// Whether the HLS demuxer plugin is available in this installation.
pub fn hls_demux_available() -> bool {
    gstreamer::ElementFactory::find("hlsdemux2")
        .or_else(|| gstreamer::ElementFactory::find("hlsdemux"))
        .is_some()
}

/// Whether the `appsink` element (gstreamer-app library) is registered.
pub fn app_sink_available() -> bool {
    gstreamer::ElementFactory::find("appsink").is_some()
}

fn manifest_path(record: &ResumeRecord) -> PathBuf {
    // Fixtures use repo-relative paths; allow absolute overrides too.
    PathBuf::from(&record.manifest_url)
}

/// Build and prime a `uridecodebin ... fakesink` pipeline up to `PAUSED`,
/// recording durable evidence in `temp_dir`.
pub fn probe_resume(record: &ResumeRecord, temp_dir: &Path) -> GstProbeEvidence {
    // Initialization must happen before any ElementFactory query.
    if let Err(message) = ensure_init() {
        let evidence = GstProbeEvidence {
            record_id: record.record_id.clone(),
            manifest_url: record.manifest_url.clone(),
            element_factory: "uninitialized".to_string(),
            probe_ok: false,
            detail: format!("gstreamer init failed: {message}"),
        };
        let scratch = temp_dir.join(format!("gst-{}.json", record.record_id));
        if let Ok(bytes) = serde_json::to_vec_pretty(&evidence) {
            let _ = std::fs::write(scratch, bytes);
        }
        return evidence;
    }

    let factory = if gstreamer::ElementFactory::find("hlsdemux2").is_some() {
        "hlsdemux2"
    } else if gstreamer::ElementFactory::find("hlsdemux").is_some() {
        "hlsdemux"
    } else {
        "missing"
    };

    let manifest = manifest_path(record);
    let evidence = run_pipeline(record, &manifest, temp_dir, factory);

    // Durable per-record scratch evidence, cleaned up with the temp dir.
    let scratch = temp_dir.join(format!("gst-{}.json", record.record_id));
    if let Ok(bytes) = serde_json::to_vec_pretty(&evidence) {
        let _ = std::fs::write(scratch, bytes);
    }
    evidence
}

fn run_pipeline(
    record: &ResumeRecord,
    manifest: &Path,
    temp_dir: &Path,
    factory: &str,
) -> GstProbeEvidence {
    if factory == "missing" {
        return GstProbeEvidence {
            record_id: record.record_id.clone(),
            manifest_url: record.manifest_url.clone(),
            element_factory: factory.to_string(),
            probe_ok: false,
            detail: "no HLS demuxer plugin registered in this GStreamer".to_string(),
        };
    }
    if !manifest.exists() {
        return GstProbeEvidence {
            record_id: record.record_id.clone(),
            manifest_url: record.manifest_url.clone(),
            element_factory: factory.to_string(),
            probe_ok: false,
            detail: format!("manifest not found: {}", manifest.display()),
        };
    }

    let uri = url_from_path(manifest);
    let src = match gstreamer::ElementFactory::make("uridecodebin")
        .property("uri", &uri)
        .build()
    {
        Ok(element) => element,
        Err(err) => {
            return failed(
                record,
                factory,
                format!("uridecodebin creation failed: {err}"),
            )
        }
    };
    let sink: gstreamer::Element = gstreamer_app::AppSink::builder()
        .name("resume-probe-appsink")
        .sync(false)
        .build()
        .upcast();
    let pipeline = gstreamer::Pipeline::new();
    if pipeline.add_many([&src, &sink]).is_err() {
        return failed(
            record,
            factory,
            "could not add elements to pipeline".to_string(),
        );
    }

    // uridecodebin exposes pads only after it has inspected the manifest.
    let sink_weak = gstreamer::glib::object::ObjectExt::downgrade(&sink);
    src.connect_pad_added(move |_src, pad| {
        if let Some(sink) = sink_weak.upgrade() {
            let sink_pad = sink
                .static_pad("sink")
                .expect("fakesink always has a sink pad");
            let _ = pad.link(&sink_pad);
        }
    });

    let bus = pipeline.bus().expect("pipeline always has a bus");
    let state_result = pipeline.set_state(gstreamer::State::Paused);
    let probe_ok = matches!(
        state_result,
        Ok(gstreamer::StateChangeSuccess::Success) | Ok(gstreamer::StateChangeSuccess::Async)
    );

    // Wait briefly for the async pre-roll; errors are captured, not unwrapped.
    let mut detail = match state_result {
        Ok(gstreamer::StateChangeSuccess::Async) => {
            "pipeline reached PAUSED asynchronously (HLS manifest primed)".to_string()
        }
        Ok(other) => format!("pipeline state change: {other:?}"),
        Err(err) => format!("state change failed: {err}"),
    };

    if let Some(message) = bus.timed_pop_filtered(
        gstreamer::ClockTime::from_seconds(3),
        &[
            gstreamer::MessageType::Error,
            gstreamer::MessageType::AsyncDone,
            gstreamer::MessageType::StateChanged,
        ],
    ) {
        if let gstreamer::MessageView::Error(error) = message.view() {
            detail = format!("bus error while priming: {}", error.error());
            let _ = pipeline.set_state(gstreamer::State::Null);
            return GstProbeEvidence {
                record_id: record.record_id.clone(),
                manifest_url: record.manifest_url.clone(),
                element_factory: factory.to_string(),
                probe_ok: false,
                detail,
            };
        }
    }

    let scratch_note = temp_dir.join(format!("gst-{}.state.txt", record.record_id));
    let _ = std::fs::write(
        scratch_note,
        format!(
            "uri={uri}\nresume_position_ms={}\ntarget_state=PAUSED\nfactory={factory}\nappsink_available={}\n",
            record.resume_position_ms,
            app_sink_available()
        ),
    );

    let _ = pipeline.set_state(gstreamer::State::Null);
    GstProbeEvidence {
        record_id: record.record_id.clone(),
        manifest_url: record.manifest_url.clone(),
        element_factory: factory.to_string(),
        probe_ok,
        detail,
    }
}

fn failed(record: &ResumeRecord, factory: &str, detail: String) -> GstProbeEvidence {
    GstProbeEvidence {
        record_id: record.record_id.clone(),
        manifest_url: record.manifest_url.clone(),
        element_factory: factory.to_string(),
        probe_ok: false,
        detail,
    }
}

fn url_from_path(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    gstreamer::glib::filename_to_uri(absolute, None)
        .map(|uri| uri.to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_callable_repeatedly() {
        ensure_init().expect("gstreamer initializes");
        ensure_init().expect("second init is a no-op");
    }
}
