//! GStreamer 独立核验：`filesrc ! subparse ! appsink`。
//!
//! 每个快照在独立 pipeline 中核验，任何错误都被收敛为 [`GstCheck`] 的字段，
//! 由上层决定是否继续处理其余快照，而不是让整个进程退出。

use std::path::Path;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use crate::model::GstCheck;

/// 进程内只初始化一次 GStreamer。
pub fn ensure_init() -> Result<(), String> {
    static INIT_RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    INIT_RESULT
        .get_or_init(|| {
            gst::init()
                .map(|_| ())
                .map_err(|err| format!("GStreamer 初始化失败: {err}"))
        })
        .clone()
}

#[derive(Debug, Default)]
struct EmittedCue {
    start_ns: u64,
    end_ns: u64,
}

/// 对单个落盘快照执行 pipeline 核验。
pub fn check_snapshot(snapshot_id: &str, path: &Path) -> GstCheck {
    check_snapshot_inner(snapshot_id, path, false)
}

/// 带故障注入的内部实现；`force_failure=true` 时在 pipeline 构建前模拟异常，
/// 用于验证单对象异常被隔离、其余对象继续处理。
fn check_snapshot_inner(snapshot_id: &str, path: &Path, force_failure: bool) -> GstCheck {
    let pipeline_desc =
        format!("filesrc location={path:?} ! subparse ! appsink name=sink emit-signals=true");
    let mut result = GstCheck {
        snapshot_id: snapshot_id.to_string(),
        pipeline_desc: pipeline_desc.clone(),
        ..Default::default()
    };

    let run = (|| -> Result<(), String> {
        ensure_init()?;
        if force_failure {
            return Err("注入故障：模拟该对象 pipeline 构建前异常".to_string());
        }

        let collected: Arc<Mutex<Vec<EmittedCue>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_callbacks = {
            let collected = Arc::clone(&collected);
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |appsink| {
                    let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Error)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let pts = buffer.pts().map(|t| t.nseconds()).unwrap_or(u64::MAX);
                    let duration = buffer.duration().map(|t| t.nseconds()).unwrap_or(0);
                    collected.lock().unwrap().push(EmittedCue {
                        start_ns: pts,
                        end_ns: pts.saturating_add(duration),
                    });
                    Ok(gst::FlowSuccess::Ok)
                })
                .build()
        };

        let appsink = gst_app::AppSink::builder().name("sink").build();
        appsink.set_callbacks(sink_callbacks);

        let filesrc = gst::ElementFactory::make("filesrc")
            .property("location", path.to_path_buf())
            .build()
            .map_err(|err| format!("创建 filesrc 失败: {err}"))?;
        let subparse = gst::ElementFactory::make("subparse")
            .build()
            .map_err(|err| format!("创建 subparse 失败: {err}"))?;

        let pipeline = gst::Pipeline::new();
        // 核验只关心 subparse 的解析结果，不需要按真实时钟调度字幕；
        // 去掉 pipeline 时钟后所有 buffer 立即推送，避免等待字幕时间戳走完。
        pipeline.use_clock(None::<&gst::SystemClock>);
        pipeline
            .add_many([&filesrc, &subparse, appsink.upcast_ref()])
            .map_err(|err| format!("挂载元素失败: {err}"))?;
        gst::Element::link_many([&filesrc, &subparse, appsink.upcast_ref()])
            .map_err(|err| format!("链接 pipeline 失败: {err}"))?;

        pipeline
            .set_state(gst::State::Playing)
            .map_err(|err| format!("进入 Playing 失败: {err}"))?;

        let bus = pipeline.bus().ok_or("pipeline 无 bus")?;
        let msg = bus.timed_pop_filtered(
            10 * gst::ClockTime::SECOND,
            &[gst::MessageType::Error, gst::MessageType::Eos],
        );

        let msg = match msg {
            Some(msg) => msg,
            None => {
                pipeline.set_state(gst::State::Null).ok();
                return Err("等待 EOS/ERROR 超时（10s）".to_string());
            }
        };

        let src_name = msg
            .src()
            .map(|s| s.name())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "<无来源>".to_string());
        result.bus_message = format!("{:?}: {src_name}", msg.type_());
        if let gst::MessageView::Error(err) = msg.view() {
            pipeline.set_state(gst::State::Null).ok();
            return Err(format!("bus ERROR: {} ({:?})", err.error(), err.debug()));
        }

        pipeline
            .set_state(gst::State::Null)
            .map_err(|err| format!("停止 pipeline 失败: {err}"))?;

        let cues = collected.lock().unwrap();
        result.emitted_cues = cues.len();
        result.first_start_ns = cues.first().map(|c| c.start_ns);
        result.last_end_ns = cues.last().map(|c| c.end_ns);
        result.success = true;
        Ok(())
    })();

    if let Err(err) = run {
        result.success = false;
        result.error = Some(err);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::TempWorkspace;

    #[test]
    fn pipeline_emits_cues_for_valid_srt() {
        let ws = TempWorkspace::create().unwrap();
        let path = ws
            .stage(
                "ok.srt",
                b"1\n00:00:01,000 --> 00:00:02,000\nhello\n\n2\n00:00:03,000 --> 00:00:04,000\nworld\n\n",
            )
            .unwrap();
        let check = check_snapshot("snapshot_a", &path);
        assert!(check.success, "GST 核验应成功: {:?}", check.error);
        assert_eq!(check.emitted_cues, 2);
        assert_eq!(check.first_start_ns, Some(1_000_000_000));
        assert_eq!(check.last_end_ns, Some(4_000_000_000));
    }

    #[test]
    fn pipeline_error_is_isolated_not_panicking() {
        let ws = TempWorkspace::create().unwrap();
        let path = ws.stage("missing.srt", b"not an srt at all").unwrap();
        let check = check_snapshot("snapshot_x", &path);
        // subparse 对纯文本可能报错或输出 0 条；两种情况都必须被收敛。
        assert!(check.error.is_some() || check.emitted_cues == 0);
    }

    #[test]
    fn injected_failure_is_captured_into_result() {
        let ws = TempWorkspace::create().unwrap();
        let path = ws
            .stage("inj.srt", b"1\n00:00:01,000 --> 00:00:02,000\nx\n\n")
            .unwrap();
        let check = super::check_snapshot_inner("snapshot_inj", &path, true);
        assert!(!check.success);
        assert!(check.error.unwrap().contains("注入故障"));
        // 故障后同一线程仍可正常核验下一个对象。
        let next = super::check_snapshot_inner("snapshot_ok", &path, false);
        assert!(
            next.success,
            "异常隔离后应继续处理其余对象: {:?}",
            next.error
        );
        assert_eq!(next.emitted_cues, 1);
    }
}
