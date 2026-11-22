//! GStreamer 帧元数据提取。
//!
//! 管线：filesrc → decodebin → videoconvert → appsink。
//! 逐 sample 读取 caps 中的色彩信息（传递函数/基色/矩阵/范围）与
//! 母版显示信息、内容亮度级别字段，组装成 `FrameMeta`。

use crate::meta::*;
use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app::AppSink;
use gstreamer_video as gst_video;

/// 从视频文件提取逐帧元数据，最多 `max_frames` 帧。
pub fn extract_frames(path: &str, max_frames: u64) -> Result<Vec<FrameMeta>> {
    gst::init().context("GStreamer 初始化失败")?;
    let pipeline_desc = format!(
        "filesrc location=\"{path}\" ! decodebin ! videoconvert ! appsink name=sink"
    );
    let pipeline = gst::parse::launch(&pipeline_desc)
        .context("无法构建解码管线")?
        .downcast::<gst::Pipeline>()
        .map_err(|_| anyhow::anyhow!("管线类型错误"))?;
    let sink = pipeline
        .by_name("sink")
        .context("找不到 appsink")?
        .downcast::<AppSink>()
        .map_err(|_| anyhow::anyhow!("appsink 类型错误"))?;

    pipeline.set_state(gst::State::Playing)?;
    let mut frames = Vec::new();
    let mut frame_no = 0u64;
    while frame_no < max_frames {
        match sink.pull_sample() {
            Ok(sample) => {
                let mut meta = FrameMeta::new(frame_no);
                if let Some(caps) = sample.caps() {
                    fill_from_caps(&mut meta, caps);
                }
                infer_format_tag(&mut meta);
                frames.push(meta);
                frame_no += 1;
            }
            Err(_) => break, // EOS 或错误均结束
        }
    }
    pipeline.set_state(gst::State::Null).ok();
    Ok(frames)
}

/// 从 caps 组装一帧元数据（含格式标签推断），单独公开以便测试。
pub fn frame_meta_from_caps(caps: &gst::CapsRef, frame_no: u64) -> FrameMeta {
    let mut meta = FrameMeta::new(frame_no);
    fill_from_caps(&mut meta, caps);
    infer_format_tag(&mut meta);
    meta
}

fn fill_from_caps(meta: &mut FrameMeta, caps: &gst::CapsRef) {
    let Ok(info) = gst_video::VideoInfo::from_caps(caps) else {
        return;
    };
    let c = info.colorimetry();
    let transfer = match c.transfer() {
        gst_video::VideoTransferFunction::Bt709 => Some(Transfer::Bt709),
        gst_video::VideoTransferFunction::Srgb => Some(Transfer::Srgb),
        gst_video::VideoTransferFunction::Smpte2084 => Some(Transfer::Pq),
        gst_video::VideoTransferFunction::AribStdB67 => Some(Transfer::Hlg),
        _ => None,
    };
    if let Some(t) = transfer {
        meta.set(FieldKey::Transfer, FieldValue::Transfer(t));
    }
    let primaries = match c.primaries() {
        gst_video::VideoColorPrimaries::Bt709 => Some(Primaries::Bt709),
        gst_video::VideoColorPrimaries::Bt2020 => Some(Primaries::Bt2020),
        _ => None,
    };
    if let Some(p) = primaries {
        meta.set(FieldKey::Primaries, FieldValue::Primaries(p));
    }
    let matrix = match c.matrix() {
        gst_video::VideoColorMatrix::Bt709 => Some(Matrix::Bt709),
        gst_video::VideoColorMatrix::Bt2020 => Some(Matrix::Bt2020Ncl),
        _ => None,
    };
    if let Some(m) = matrix {
        meta.set(FieldKey::Matrix, FieldValue::Matrix(m));
    }
    let range = match c.range() {
        gst_video::VideoColorRange::Range0_255 => Some(Range::Full),
        gst_video::VideoColorRange::Range16_235 => Some(Range::Limited),
        _ => None,
    };
    if let Some(r) = range {
        meta.set(FieldKey::Range, FieldValue::Range(r));
    }
    // caps 上携带的静态 HDR 元数据（ST 2086 / CTA-861.3）。
    if let Ok(md) = gst_video::VideoMasteringDisplayInfo::from_caps(caps) {
        meta.set(
            FieldKey::MasteringMaxLum,
            FieldValue::Number(md.max_display_mastering_luminance() as f64),
        );
        meta.set(
            FieldKey::MasteringMinLum,
            FieldValue::Number(md.min_display_mastering_luminance() as f64 / 10000.0),
        );
    }
    if let Ok(cll) = gst_video::VideoContentLightLevel::from_caps(caps) {
        meta.set(
            FieldKey::MaxCll,
            FieldValue::Number(cll.max_content_light_level() as f64),
        );
        meta.set(
            FieldKey::MaxFall,
            FieldValue::Number(cll.max_frame_average_light_level() as f64),
        );
    }
}

/// 码流里没有显式 HDR 格式标签时，按传递函数推断，供规则引擎继续检查。
fn infer_format_tag(meta: &mut FrameMeta) {
    if meta.get(FieldKey::HdrFormat).is_some() {
        return;
    }
    let tag = match meta.get(FieldKey::Transfer) {
        Some(FieldValue::Transfer(Transfer::Pq)) => Some(HdrFormat::Hdr10),
        Some(FieldValue::Transfer(Transfer::Hlg)) => Some(HdrFormat::Hlg),
        Some(FieldValue::Transfer(_)) => Some(HdrFormat::Sdr),
        _ => None,
    };
    if let Some(t) = tag {
        meta.set(FieldKey::HdrFormat, FieldValue::Format(t));
    }
}
