use crate::model::Rotation;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::Path;
use std::sync::Once;

static GST_INIT: Once = Once::new();

/// 初始化 GStreamer（幂等）。
pub fn ensure_init() -> Result<(), String> {
    let mut result = Ok(());
    GST_INIT.call_once(|| {
        // 插件来自本地 sysroot（coreelements + typefindfunctions）。
        if std::env::var_os("GST_PLUGIN_SYSTEM_PATH").is_none() {
            if let Ok(sysroot) = std::env::var("ROTFIX_SYSROOT") {
                let plugins = format!("{sysroot}/gst-plugins-rotfix");
                std::env::set_var("GST_PLUGIN_PATH", &plugins);
                std::env::set_var("GST_PLUGIN_SYSTEM_PATH", plugins);
            }
        }
        if let Err(e) = gst::init() {
            result = Err(format!("gst::init 失败: {e}"));
        }
    });
    result
}

/// 用 filesrc ! typefind 探测容器类型，返回 caps 字符串。
pub fn probe_container(media: &Path) -> Result<String, String> {
    ensure_init()?;
    let pipeline = gst::Pipeline::new();
    let src = gst::ElementFactory::make("filesrc")
        .property("location", media.to_string_lossy().as_ref())
        .build()
        .map_err(|e| format!("创建 filesrc 失败: {e}"))?;
    let typefind = gst::ElementFactory::make("typefind")
        .build()
        .map_err(|e| format!("创建 typefind 失败: {e}"))?;
    let sink = gst::ElementFactory::make("fakesink")
        .build()
        .map_err(|e| format!("创建 fakesink 失败: {e}"))?;
    pipeline
        .add_many([&src, &typefind, &sink])
        .and_then(|_| gst::Element::link_many([&src, &typefind, &sink]))
        .map_err(|e| format!("组装探测管线失败: {e}"))?;

    let (tx, rx) = std::sync::mpsc::channel::<String>();
    typefind.connect("have-type", false, move |values| {
        let caps = values[2].get::<gst::Caps>().ok();
        if let Some(caps) = caps {
            let _ = tx.send(caps.to_string());
        }
        None
    });

    pipeline
        .set_state(gst::State::Playing)
        .map_err(|e| format!("启动探测管线失败: {e}"))?;
    let bus = pipeline.bus();
    let mut found = rx.try_recv().ok();
    if found.is_none() {
        if let Some(bus) = bus {
            for msg in bus.iter_timed(gst::ClockTime::from_seconds(5)) {
                use gst::MessageView;
                if let Ok(caps) = rx.try_recv() {
                    found = Some(caps);
                    break;
                }
                match msg.view() {
                    MessageView::Eos(..) => break,
                    MessageView::Error(err) => {
                        pipeline.set_state(gst::State::Null).ok();
                        return Err(format!("探测管线错误: {}", err.error()));
                    }
                    _ => {}
                }
            }
        }
    }
    pipeline.set_state(gst::State::Null).ok();
    found.ok_or_else(|| "typefind 未识别容器类型".to_string())
}

/// 用 GStreamer TagList 规范化旋转标签：
/// 把原始标签写入 TagList 再读回，得到标准 image-orientation 值。
pub fn normalize_orientation_tag(raw: &str) -> Option<Rotation> {
    ensure_init().ok()?;
    let mut tags = gst::TagList::new();
    tags.get_mut()
        .unwrap()
        .add::<gst::tags::ImageOrientation>(&raw, gst::TagMergeMode::Replace);
    let readback: String = tags
        .get::<gst::tags::ImageOrientation>()
        .map(|v| v.get().to_string())?;
    Rotation::from_orientation_tag(&readback)
}
