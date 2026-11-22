//! 快照解析：兼容旧格式（legacy，扁平 JSON）与新格式（v2，分版记录）。

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 快照解析错误。
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("快照 JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),
    #[error("快照缺少可识别的格式标记（既不是 legacy 也不是 v2）")]
    UnknownFormat,
    #[error("对象 {0} 缺少媒体路径")]
    MissingMedia(String),
}

/// 统一的待处理记录，屏蔽两种快照格式的差异。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotRecord {
    /// 稳定对象标识。
    pub object_id: String,
    /// 媒体文件路径。
    pub media_path: String,
    /// 轨道或帧引用（原样保留）。
    pub track_ref: String,
    /// 记录是否被截断（缺少明确结束标记）。
    pub truncated: bool,
    /// 来源格式：`legacy` / `v2`。
    pub source_format: &'static str,
}

/// 旧格式：扁平数组，字段名简短，截断记录缺少 `end_time`/`closed` 标记。
#[derive(Debug, Deserialize)]
struct LegacyRecord {
    id: String,
    file: String,
    /// 旧格式用 "track" 或 "frame" 描述轨道/帧，二者至少出现其一。
    track: Option<String>,
    frame: Option<u64>,
    /// 旧格式里存在该字段且为 true 表示记录完整；缺失或 false 表示截断。
    closed: Option<bool>,
    end_time: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct LegacySnapshot {
    #[serde(default)]
    records: Vec<LegacyRecord>,
}

/// 新格式：带显式版本号，记录分对象组织，截断用独立状态表达。
#[derive(Debug, Deserialize)]
struct V2Snapshot {
    #[allow(dead_code)]
    version: u32,
    objects: Vec<V2Object>,
}

#[derive(Debug, Deserialize)]
struct V2Object {
    object_id: String,
    media: V2Media,
    /// v2 的生命周期状态；`truncated` 为显式截断。
    #[serde(default)]
    state: String,
}

#[derive(Debug, Deserialize)]
struct V2Media {
    path: String,
    track: String,
    /// v2 提供精确帧引用。
    start_frame: u64,
    end_frame: Option<u64>,
}

/// 自动识别格式并解析为统一记录列表。
pub fn parse_snapshot(json_text: &str) -> Result<Vec<SnapshotRecord>, SnapshotError> {
    let value: serde_json::Value = serde_json::from_str(json_text)?;

    if let Some(v2) = value.get("version").and_then(|v| v.as_u64()) {
        if v2 == 2 {
            return parse_v2(value);
        }
    }
    if value.get("records").is_some() {
        return parse_legacy(value);
    }
    Err(SnapshotError::UnknownFormat)
}

fn parse_legacy(value: serde_json::Value) -> Result<Vec<SnapshotRecord>, SnapshotError> {
    let snap: LegacySnapshot = serde_json::from_value(value)?;
    snap.records
        .into_iter()
        .map(|r| {
            if r.file.trim().is_empty() {
                return Err(SnapshotError::MissingMedia(r.id));
            }
            // 旧格式没有显式截断字段：closed != true 或缺少 end_time 即判定为截断。
            let truncated = r.closed != Some(true) || r.end_time.is_none();
            let track_ref = match (r.track.as_deref(), r.frame) {
                (Some(t), Some(f)) => format!("track={t};frame={f}"),
                (Some(t), None) => format!("track={t}"),
                (None, Some(f)) => format!("frame={f}"),
                (None, None) => "track=unknown".to_string(),
            };
            Ok(SnapshotRecord {
                object_id: r.id,
                media_path: r.file,
                track_ref,
                truncated,
                source_format: "legacy",
            })
        })
        .collect()
}

fn parse_v2(value: serde_json::Value) -> Result<Vec<SnapshotRecord>, SnapshotError> {
    let snap: V2Snapshot = serde_json::from_value(value)?;
    snap.objects
        .into_iter()
        .map(|o| {
            if o.media.path.trim().is_empty() {
                return Err(SnapshotError::MissingMedia(o.object_id));
            }
            let truncated =
                o.state.eq_ignore_ascii_case("truncated") || o.media.end_frame.is_none();
            let track_ref = format!(
                "track={};start_frame={};end_frame={}",
                o.media.track,
                o.media.start_frame,
                o.media
                    .end_frame
                    .map(|f| f.to_string())
                    .unwrap_or_else(|| "open".to_string())
            );
            Ok(SnapshotRecord {
                object_id: o.object_id,
                media_path: o.media.path,
                track_ref,
                truncated,
                source_format: "v2",
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY: &str = r#"{
        "records": [
            {"id": "L1", "file": "a.wav", "track": "vocals", "frame": 0, "closed": true, "end_time": 4.0},
            {"id": "L2-TRUNC", "file": "b.wav", "track": "mic", "frame": 1280}
        ]
    }"#;

    const V2: &str = r#"{
        "version": 2,
        "objects": [
            {"object_id": "N1", "state": "complete",
             "media": {"path": "c.wav", "track": "master", "start_frame": 0, "end_frame": 192000}},
            {"object_id": "N2-TRUNC", "state": "truncated",
             "media": {"path": "d.wav", "track": "boom", "start_frame": 48000}}
        ]
    }"#;

    #[test]
    fn parses_legacy_and_marks_truncation() {
        let recs = parse_snapshot(LEGACY).unwrap();
        assert_eq!(recs.len(), 2);
        assert!(!recs[0].truncated);
        assert_eq!(recs[0].source_format, "legacy");
        assert!(recs[1].truncated);
        assert!(recs[1].track_ref.contains("frame=1280"));
    }

    #[test]
    fn parses_v2_and_marks_truncation() {
        let recs = parse_snapshot(V2).unwrap();
        assert_eq!(recs.len(), 2);
        assert!(!recs[0].truncated);
        assert_eq!(recs[0].source_format, "v2");
        assert!(recs[1].truncated);
        assert!(recs[1].track_ref.contains("end_frame=open"));
    }

    #[test]
    fn rejects_unknown_format() {
        assert!(parse_snapshot(r#"{"hello": 1}"#).is_err());
    }
}
