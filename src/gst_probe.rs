use std::time::{Duration, Instant};

use gst::prelude::*;

#[derive(Clone, Debug)]
pub struct ProbeReport {
    pub peak_ms: u64,
    pub samples: usize,
    pub duration_ms: u128,
    pub threshold_ms: u64,
}

#[derive(Clone, Debug)]
pub struct SourceReport {
    pub preroll_ms: u128,
    pub state: String,
}

const PROBE_THRESHOLD_MS: u64 = 1_500;

pub fn run_backpressure_probe() -> Result<ProbeReport, String> {
    let description = concat!(
        "videotestsrc is-live=true num-buffers=120 ! ",
        "video/x-raw,width=1280,height=720,framerate=30/1 ! ",
        "queue name=probe_queue max-size-time=5000000000 max-size-buffers=0 max-size-bytes=0 ! ",
        "identity sleep-time=55000 ! fakesink sync=false"
    );
    let pipeline = gst::parse::launch(description)
        .map_err(|error| format!("无法创建探针管线: {error}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "探针管线不是 GstPipeline".to_string())?;
    let queue = pipeline
        .by_name("probe_queue")
        .ok_or_else(|| "找不到 queue 探针".to_string())?;

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|error| format!("探针无法进入 PLAYING: {error:?}"))?;

    let started = Instant::now();
    let mut peak_ms = 0_u64;
    let mut samples = 0_usize;

    while started.elapsed() < Duration::from_millis(2_650) {
        std::thread::sleep(Duration::from_millis(40));
        let level = queue
            .property_value("current-level-time")
            .get::<u64>()
            .unwrap_or(0);
        peak_ms = peak_ms.max(level / 1_000_000);
        samples += 1;
    }

    let _ = pipeline.set_state(gst::State::Null);
    let duration_ms = started.elapsed().as_millis();

    Ok(ProbeReport {
        peak_ms,
        samples,
        duration_ms,
        threshold_ms: PROBE_THRESHOLD_MS,
    })
}

pub fn validate_source(uri: &str) -> Result<SourceReport, String> {
    let escaped = uri.replace('\\', "\\\\").replace('"', "\\\"");
    let description = format!("uridecodebin uri=\"{escaped}\" ! fakesink sync=false");
    let pipeline = gst::parse::launch(&description)
        .map_err(|error| format!("无法创建来源探针: {error}"))?
        .downcast::<gst::Pipeline>()
        .map_err(|_| "来源探针不是 GstPipeline".to_string())?;

    let bus = pipeline
        .bus()
        .ok_or_else(|| "来源探针缺少 bus".to_string())?;
    pipeline.set_state(gst::State::Paused).map_err(|error| {
        format!("来源不可达，探针无法进入 PAUSED: {error:?}；请检查 URL、访问权限和网络")
    })?;

    let started = Instant::now();
    let deadline = Duration::from_millis(3_000);
    let mut last_state = gst::State::Ready;

    while started.elapsed() < deadline {
        if let Some(message) = bus.timed_pop_filtered(
            gst::ClockTime::from_mseconds(100),
            &[
                gst::MessageType::Error,
                gst::MessageType::AsyncDone,
                gst::MessageType::StateChanged,
            ],
        ) {
            match message.view() {
                gst::MessageView::Error(error) => {
                    let _ = pipeline.set_state(gst::State::Null);
                    return Err(format!(
                        "来源不可达: {} ({})",
                        error.error(),
                        error.debug().unwrap_or_default()
                    ));
                }
                gst::MessageView::AsyncDone(_) => {
                    let caps = pipeline.current_state().to_string();
                    let _ = pipeline.set_state(gst::State::Null);
                    return Ok(SourceReport {
                        preroll_ms: started.elapsed().as_millis(),
                        state: caps,
                    });
                }
                gst::MessageView::StateChanged(changed) => {
                    last_state = changed.current();
                }
                _ => {}
            }
        }
    }

    let _ = pipeline.set_state(gst::State::Null);
    Err(format!(
        "来源在 3 秒内未完成 preroll（停留于 {last_state:?}），请检查 DNS、签名 URL 或边缘节点健康度"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_probe_observes_queue_growth() {
        let _ = gst::init();
        let report = run_backpressure_probe().expect("local GStreamer probe should run");
        assert!(report.samples > 10);
        assert!(
            report.peak_ms > 0,
            "queue should buffer behind the slow consumer"
        );
        assert!(report.duration_ms >= 2_000);
    }

    #[test]
    fn missing_source_returns_actionable_error() {
        let _ = gst::init();
        let error = validate_source("file:///tmp/backpressure-inspector-missing-source.ts")
            .expect_err("missing source should not preroll");
        assert!(
            error.contains("来源不可达") || error.contains("preroll"),
            "unexpected error: {error}"
        );
    }
}
