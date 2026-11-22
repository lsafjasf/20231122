use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::discovery::ResolvedManifest;
use crate::failure::{Failure, Result};
use crate::sha256;

#[derive(Debug, Clone, Serialize)]
pub struct SchemaInfo {
    pub detected_format: String,
    pub input_version: String,
    pub normalized_version: u32,
    pub migrated: bool,
    pub manifest_sha256: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseInfo {
    pub case_id: String,
    pub recovered_at: Option<String>,
    pub replay_origin: String,
    pub source_name: Option<String>,
    pub source_note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TrackInfo {
    pub id: String,
    pub index: u64,
    pub name: String,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub frame_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProcessingPreset {
    pub id: String,
    pub name: String,
    pub parameters: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LossEvent {
    pub id: String,
    pub sequence: u64,
    pub kind: String,
    pub start_ms: f64,
    pub duration_ms: Option<f64>,
    pub end_ms: Option<f64>,
    pub track_id: Option<String>,
    pub track_index: Option<u64>,
    pub frame: Option<u64>,
    pub processing_id: Option<String>,
    pub parameters: BTreeMap<String, Value>,
    pub effective_parameters: BTreeMap<String, Value>,
    pub raw: Value,
}

impl LossEvent {
    pub fn end_or_start_ms(&self) -> f64 {
        self.end_ms
            .or_else(|| self.duration_ms.map(|duration| self.start_ms + duration))
            .unwrap_or(self.start_ms)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseManifest {
    pub schema: SchemaInfo,
    pub case: CaseInfo,
    pub audio_path: PathBuf,
    pub tracks: Vec<TrackInfo>,
    pub processing: Vec<ProcessingPreset>,
    pub events: Vec<LossEvent>,
    pub warnings: Vec<String>,
}

pub fn load_or_infer(
    resolved: Option<ResolvedManifest>,
    audio_override: Option<&Path>,
) -> Result<CaseManifest> {
    match resolved {
        Some(resolved) => parse_manifest(&resolved, audio_override),
        None => infer_manifest(audio_override.ok_or_else(|| {
            Failure::new(
                "missing_input",
                "没有可评估的音频或清单",
                "传入 `--manifest <清单.json>`，或传入 `--audio <重放文件>`。",
            )
        })?),
    }
}

fn infer_manifest(audio: &Path) -> Result<CaseManifest> {
    let now = chrono::Utc::now().to_rfc3339();
    let case_id = format!("inferred-{}", chrono::Utc::now().format("%Y%m%dT%H%M%SZ"));
    Ok(CaseManifest {
        schema: SchemaInfo {
            detected_format: "inferred".to_string(),
            input_version: "none".to_string(),
            normalized_version: 3,
            migrated: false,
            manifest_sha256: sha256::bytes_sha256(b"{}"),
        },
        case: CaseInfo {
            case_id,
            recovered_at: Some(now),
            replay_origin: "recovery replay start (0:00)".to_string(),
            source_name: audio
                .file_name()
                .and_then(|value| value.to_str())
                .map(str::to_string),
            source_note: Some("未找到清单，已从重放音频建立最小案件。".to_string()),
        },
        audio_path: audio.to_path_buf(),
        tracks: vec![TrackInfo {
            id: "recovered-main".to_string(),
            index: 0,
            name: "Recovered replay".to_string(),
            codec: None,
            sample_rate: None,
            channels: None,
            frame_size: None,
        }],
        processing: Vec::new(),
        events: Vec::new(),
        warnings: vec![
            "未找到评估清单；轨道、帧号和丢包事件均未知。".to_string(),
            "自动结论不能把缺失事件解释为通过，必须人工确认或补充清单。".to_string(),
        ],
    })
}

fn parse_manifest(
    resolved: &ResolvedManifest,
    audio_override: Option<&Path>,
) -> Result<CaseManifest> {
    let raw: Value = serde_json::from_slice(&resolved.bytes)
        .map_err(|e| Failure::json("评估清单", &resolved.path, e))?;
    let (root_value, was_bare_events) = match &raw {
        Value::Object(_) => (raw.clone(), false),
        Value::Array(_) => {
            let mut map = Map::new();
            map.insert("events".to_string(), raw.clone());
            (Value::Object(map), true)
        }
        _ => {
            return Err(Failure::new(
                "manifest_shape",
                "评估清单必须是 JSON 对象或旧格式事件数组",
                "把顶层改为对象，或使用 `{ \"events\": [...] }` 结构后重试。",
            ));
        }
    };
    let root = root_value.as_object().ok_or_else(|| {
        Failure::new(
            "manifest_shape",
            "评估清单顶层结构无效",
            "修复 JSON 顶层结构后重试。",
        )
    })?;

    let mut warnings = Vec::new();
    let (detected_format, input_version, migrated) = detect_schema(root, was_bare_events);
    let legacy = detected_format == "legacy";
    let manifest_dir = resolved.path.parent().unwrap_or_else(|| Path::new("."));
    let audio_path = resolve_audio_path(root, manifest_dir, audio_override)?;
    let tracks = parse_tracks(root, &mut warnings);
    let processing = parse_processing(root, &mut warnings);
    let mut events = parse_events(root, legacy, &mut warnings)?;
    link_events(&tracks, &processing, &mut events, &mut warnings);
    events.sort_by(|left, right| {
        left.start_ms
            .partial_cmp(&right.start_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (index, event) in events.iter_mut().enumerate() {
        event.sequence = index as u64 + 1;
    }

    if events.is_empty() {
        warnings.push("清单中没有丢包/掩蔽事件；无法仅凭音频自动判定通过。".to_string());
    }
    if !resolved.candidates.is_empty() && resolved.candidates.len() > 1 {
        warnings.push(format!(
            "发现多个候选清单，已采用 {}；其余候选：{}",
            resolved.path.display(),
            resolved
                .candidates
                .iter()
                .skip(1)
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let case_source = root.get("source");
    Ok(CaseManifest {
        schema: SchemaInfo {
            detected_format,
            input_version,
            normalized_version: 3,
            migrated,
            manifest_sha256: sha256::bytes_sha256(&resolved.bytes),
        },
        case: CaseInfo {
            case_id: string_at(root, &["case_id", "session_id", "id"]).unwrap_or_else(|| {
                resolved
                    .path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or("case")
                    .to_string()
            }),
            recovered_at: string_at(root, &["recovered_at", "created_at", "timestamp"]),
            replay_origin: string_at(root, &["replay_origin", "time_origin"])
                .unwrap_or_else(|| "recovery replay start (0:00)".to_string()),
            source_name: case_source
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| string_at(root, &["source_name", "ticket"])),
            source_note: case_source
                .and_then(|value| value.get("note"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| string_at(root, &["note", "description"])),
        },
        audio_path,
        tracks,
        processing,
        events,
        warnings,
    })
}

fn detect_schema(root: &Map<String, Value>, bare_events: bool) -> (String, String, bool) {
    if bare_events {
        return ("legacy".to_string(), "array-v0".to_string(), true);
    }
    let version_value = root.get("format_version").or_else(|| root.get("version"));
    let version = version_value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
        .unwrap_or(1);
    let marker = root
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if version >= 2 || marker.contains("assessment") {
        ("new".to_string(), format!("v{version}"), false)
    } else {
        ("legacy".to_string(), format!("v{version}"), true)
    }
}

fn resolve_audio_path(
    root: &Map<String, Value>,
    manifest_dir: &Path,
    audio_override: Option<&Path>,
) -> Result<PathBuf> {
    if let Some(path) = audio_override {
        return Ok(path.to_path_buf());
    }
    let audio_value = root.get("audio");
    let candidate = audio_value
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| {
            audio_value
                .and_then(|value| value.get("path"))
                .and_then(Value::as_str)
                .map(PathBuf::from)
        });
    let candidate = candidate.or_else(|| {
        string_at(
            root,
            &["audio_path", "file", "replay", "recovered_replay", "path"],
        )
        .map(PathBuf::from)
    });
    let Some(candidate) = candidate else {
        return Err(Failure::new(
            "manifest_audio_missing",
            "评估清单未声明重放音频路径",
            "在清单中加入 `audio.path`，或使用 `--audio <重放文件>` 覆盖。",
        ));
    };
    Ok(if candidate.is_absolute() {
        candidate
    } else {
        manifest_dir.join(candidate)
    })
}

fn parse_tracks(root: &Map<String, Value>, warnings: &mut Vec<String>) -> Vec<TrackInfo> {
    let mut tracks = Vec::new();
    let values = root
        .get("tracks")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| root.get("track").map(|value| vec![value.clone()]))
        .unwrap_or_default();

    for (position, value) in values.iter().enumerate() {
        let Some(object) = value.as_object() else {
            warnings.push(format!("忽略无法识别的轨道条目 #{position}"));
            continue;
        };
        let index = u64_at(object, &["index", "track_index", "number"]).unwrap_or(position as u64);
        let id = value_as_id(object.get("id"))
            .or_else(|| value_as_id(object.get("track_id")))
            .unwrap_or_else(|| index.to_string());
        tracks.push(TrackInfo {
            id,
            index,
            name: string_at(object, &["name", "label", "title"])
                .unwrap_or_else(|| format!("Track {index}")),
            codec: string_at(object, &["codec", "encoding", "format"]),
            sample_rate: u64_at(object, &["sample_rate", "rate", "sample_rate_hz"])
                .and_then(|value| u32::try_from(value).ok()),
            channels: u64_at(object, &["channels", "channel_count"])
                .and_then(|value| u32::try_from(value).ok()),
            frame_size: u64_at(
                object,
                &["frame_size", "samples_per_frame", "frame_samples"],
            ),
        });
    }

    if tracks.is_empty() {
        tracks.push(TrackInfo {
            id: "main".to_string(),
            index: 0,
            name: string_at(root, &["track_name", "channel_name"])
                .unwrap_or_else(|| "Main audio".to_string()),
            codec: string_at(root, &["codec", "encoding"]),
            sample_rate: u64_at(root, &["sample_rate", "rate", "sample_rate_hz"])
                .and_then(|value| u32::try_from(value).ok()),
            channels: u64_at(root, &["channels", "channel_count"])
                .and_then(|value| u32::try_from(value).ok()),
            frame_size: u64_at(root, &["frame_size", "samples_per_frame"]),
        });
    }
    tracks
}

fn parse_processing(
    root: &Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Vec<ProcessingPreset> {
    let mut presets = Vec::new();
    let mut push_preset = |value: &Value, fallback_id: String| {
        let Some(object) = value.as_object() else {
            warnings.push("忽略无法识别的处理参数条目".to_string());
            return;
        };
        let id = value_as_id(object.get("id"))
            .or_else(|| value_as_id(object.get("preset_id")))
            .unwrap_or(fallback_id);
        let name = string_at(object, &["name", "algorithm", "label"])
            .unwrap_or_else(|| "packet-loss-concealment".to_string());
        let parameters = extract_parameters(object);
        presets.push(ProcessingPreset {
            id,
            name,
            parameters,
        });
    };

    let source = root
        .get("processing")
        .or_else(|| root.get("processings"))
        .or_else(|| root.get("plc"));
    match source {
        Some(Value::Array(values)) => {
            for (index, value) in values.iter().enumerate() {
                push_preset(value, format!("processing-{}", index + 1));
            }
        }
        Some(Value::Object(_)) => push_preset(source.unwrap(), "processing-1".to_string()),
        Some(Value::String(name)) => {
            presets.push(ProcessingPreset {
                id: "processing-1".to_string(),
                name: name.clone(),
                parameters: extract_parameters(root),
            });
        }
        Some(_) => warnings.push("`processing` 字段类型无法识别".to_string()),
        None => {
            let parameters = extract_parameters(root);
            if !parameters.is_empty() {
                presets.push(ProcessingPreset {
                    id: "global-params".to_string(),
                    name: string_at(root, &["plc_name", "processing_name"])
                        .unwrap_or_else(|| "packet-loss-concealment".to_string()),
                    parameters,
                });
            }
        }
    }
    presets
}

fn parse_events(
    root: &Map<String, Value>,
    legacy_number_time_is_seconds: bool,
    warnings: &mut Vec<String>,
) -> Result<Vec<LossEvent>> {
    let Some(values) = root
        .get("events")
        .or_else(|| root.get("losses"))
        .or_else(|| root.get("packet_losses"))
    else {
        return Ok(Vec::new());
    };
    let Some(values) = values.as_array() else {
        return Err(Failure::new(
            "manifest_events_shape",
            "`events` 必须是数组",
            "将事件改为数组，例如 `\"events\": [{\"timestamp_ms\": 1850}]`。",
        ));
    };

    let mut events = Vec::new();
    for (position, value) in values.iter().enumerate() {
        let Some(object) = value.as_object() else {
            warnings.push(format!("忽略无法识别的事件 #{position}"));
            continue;
        };
        let start_ms = parse_time_ms(object, legacy_number_time_is_seconds);
        let Some(start_ms) = start_ms else {
            warnings.push(format!(
                "事件 #{} 缺少可解析的时间点，未纳入波形对照",
                position + 1
            ));
            continue;
        };
        let duration_ms = parse_duration_ms(object);
        let end_ms = parse_end_ms(object);
        let kind = event_kind(object);
        let explicit_track = value_as_id(
            object
                .get("track_id")
                .or_else(|| object.get("track"))
                .or_else(|| object.get("channel")),
        );
        let track_index = u64_at(object, &["track_index", "track_no", "channel_index"]);
        let event_parameters = extract_parameters(object);
        let frame = u64_at(
            object,
            &[
                "frame",
                "frame_index",
                "frame_number",
                "rtp_seq",
                "sequence",
            ],
        );
        events.push(LossEvent {
            id: value_as_id(object.get("id"))
                .or_else(|| value_as_id(object.get("event_id")))
                .unwrap_or_else(|| format!("event-{:03}", position + 1)),
            sequence: position as u64 + 1,
            kind,
            start_ms,
            duration_ms,
            end_ms,
            track_id: explicit_track,
            track_index,
            frame,
            processing_id: value_as_id(
                object
                    .get("processing_id")
                    .or_else(|| object.get("preset_id"))
                    .or_else(|| match object.get("processing") {
                        Some(Value::String(_)) => object.get("processing"),
                        _ => None,
                    }),
            ),
            parameters: event_parameters,
            effective_parameters: BTreeMap::new(),
            raw: value.clone(),
        });
    }
    Ok(events)
}

fn link_events(
    tracks: &[TrackInfo],
    processing: &[ProcessingPreset],
    events: &mut [LossEvent],
    warnings: &mut Vec<String>,
) {
    for event in events {
        if event.track_id.is_none() && tracks.len() == 1 {
            event.track_id = Some(tracks[0].id.clone());
            event.track_index = Some(tracks[0].index);
        }
        if let Some(track_id) = event.track_id.clone() {
            if let Some(track) = tracks
                .iter()
                .find(|track| track.id == track_id || track.index.to_string() == track_id)
            {
                event.track_id = Some(track.id.clone());
                event.track_index = Some(track.index);
            } else {
                warnings.push(format!("事件 {} 引用未知轨道 `{}`", event.id, track_id));
            }
        }

        if event.processing_id.is_none() && processing.len() == 1 {
            event.processing_id = Some(processing[0].id.clone());
        }
        event.effective_parameters = event.parameters.clone();
        if let Some(processing_id) = &event.processing_id {
            if let Some(preset) = processing.iter().find(|item| item.id == *processing_id) {
                let mut merged = preset.parameters.clone();
                merged.extend(event.parameters.clone());
                event.effective_parameters = merged;
            } else {
                warnings.push(format!(
                    "事件 {} 引用未知处理参数 `{}`",
                    event.id, processing_id
                ));
            }
        }
        if event.effective_parameters.is_empty() {
            warnings.push(format!("事件 {} 未记录处理参数", event.id));
        }
    }
}

fn event_kind(object: &Map<String, Value>) -> String {
    let lost = object
        .get("lost")
        .or_else(|| object.get("loss"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if lost {
        return "packet_loss".to_string();
    }
    string_at(object, &["kind", "type", "event"]).unwrap_or_else(|| "packet_loss".to_string())
}

fn parse_time_ms(object: &Map<String, Value>, legacy_number_is_seconds: bool) -> Option<f64> {
    for key in ["timestamp_ms", "time_ms", "start_ms", "offset_ms", "at_ms"] {
        if let Some(value) = object.get(key).and_then(Value::as_f64) {
            return Some(value);
        }
    }
    for key in ["timestamp_s", "time_s", "seconds"] {
        if let Some(value) = object.get(key).and_then(Value::as_f64) {
            return Some(value * 1000.0);
        }
    }
    for key in ["timestamp", "time", "t", "start", "offset", "at"] {
        if let Some(value) = object.get(key) {
            if let Some(text) = value.as_str() {
                return parse_time_text(text);
            }
            if let Some(number) = value.as_f64() {
                return Some(if legacy_number_is_seconds {
                    number * 1000.0
                } else {
                    number
                });
            }
        }
    }
    None
}

fn parse_duration_ms(object: &Map<String, Value>) -> Option<f64> {
    for key in ["duration_ms", "length_ms", "span_ms"] {
        if let Some(value) = object.get(key).and_then(Value::as_f64) {
            return Some(value);
        }
    }
    for key in ["duration_s", "length_s"] {
        if let Some(value) = object.get(key).and_then(Value::as_f64) {
            return Some(value * 1000.0);
        }
    }
    if let Some(value) = object.get("duration").or_else(|| object.get("length")) {
        if let Some(number) = value.as_f64() {
            return Some(number * 1000.0);
        }
        if let Some(text) = value.as_str()
            && let Some(number) = parse_time_text(text)
        {
            return Some(number);
        }
    }
    None
}

fn parse_end_ms(object: &Map<String, Value>) -> Option<f64> {
    if let Some(value) = object.get("end_ms").and_then(Value::as_f64) {
        return Some(value);
    }
    match object.get("end") {
        Some(Value::String(text)) => parse_time_text(text),
        Some(value) => value.as_f64().map(|seconds| seconds * 1000.0),
        None => None,
    }
}

fn parse_time_text(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Some(value) = text.strip_suffix("ms") {
        return value.trim().parse::<f64>().ok();
    }
    if let Some(value) = text.strip_suffix('s') {
        return value
            .trim()
            .parse::<f64>()
            .ok()
            .map(|seconds| seconds * 1000.0);
    }
    let parts: Vec<_> = text.split(':').collect();
    if parts.len() == 3 {
        let hours = parts[0].parse::<f64>().ok()?;
        let minutes = parts[1].parse::<f64>().ok()?;
        let seconds = parts[2].parse::<f64>().ok()?;
        return Some(((hours * 60.0 + minutes) * 60.0 + seconds) * 1000.0);
    }
    if parts.len() == 2 {
        let minutes = parts[0].parse::<f64>().ok()?;
        let seconds = parts[1].parse::<f64>().ok()?;
        return Some((minutes * 60.0 + seconds) * 1000.0);
    }
    text.parse::<f64>().ok().map(|seconds| seconds * 1000.0)
}

fn extract_parameters(object: &Map<String, Value>) -> BTreeMap<String, Value> {
    let mut parameters = BTreeMap::new();
    for key in [
        "parameters",
        "params",
        "plc_params",
        "processing_parameters",
        "config",
    ] {
        if let Some(Value::Object(values)) = object.get(key) {
            for (name, value) in values {
                parameters.insert(name.clone(), value.clone());
            }
        }
    }
    if let Some(Value::Object(values)) = object.get("processing") {
        for (name, value) in values {
            parameters.insert(name.clone(), value.clone());
        }
    }
    for key in ["fade_ms", "crossfade_ms", "attenuation_db", "algorithm"] {
        if let Some(value) = object.get(key) {
            parameters.insert(key.to_string(), value.clone());
        }
    }
    parameters
}

fn string_at(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object.get(*key).and_then(|value| match value {
            Value::String(text) if !text.is_empty() => Some(text.clone()),
            _ => None,
        })
    })
}

fn u64_at(object: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        object.get(*key).and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
    })
}

fn value_as_id(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

pub fn manifest_json(manifest: &CaseManifest, raw: Option<&Value>) -> Result<Value> {
    let mut value = serde_json::to_value(manifest).map_err(|error| {
        Failure::new(
            "serialize_manifest",
            "归一化清单序列化失败",
            "保留原始清单并报告此错误；这通常表示程序内部数据异常。",
        )
        .with_detail(error.to_string())
    })?;
    if let Some(raw) = raw
        && let Value::Object(object) = &mut value
    {
        object.insert("raw_manifest".to_string(), raw.clone());
    }
    Ok(value)
}

pub fn format_parameters(parameters: &BTreeMap<String, Value>) -> String {
    if parameters.is_empty() {
        return "未记录".to_string();
    }
    let object = Value::Object(
        parameters
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    );
    serde_json::to_string(&object).unwrap_or_else(|_| json!({}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(raw: &str) -> ResolvedManifest {
        ResolvedManifest {
            path: PathBuf::from("/tmp/case/manifest.json"),
            bytes: raw.as_bytes().to_vec(),
            candidates: vec![PathBuf::from("/tmp/case/manifest.json")],
        }
    }

    #[test]
    fn parses_new_manifest_and_merges_parameters() {
        let raw = r#"{
          "format": "audio-plc-assessment",
          "format_version": 2,
          "case_id": "case-new",
          "recovered_at": "2026-11-22T03:20:00Z",
          "audio": {"path": "recovered_replay.rplx"},
          "tracks": [{"id": "mic", "index": 2, "name": "Caller", "frame_size": 960}],
          "processing": [{"id": "plc-a", "name": "wsola",
            "parameters": {"fade_ms": 12, "algorithm": "wsola"}}],
          "events": [{"id": "loss-1", "timestamp_ms": 1850, "duration_ms": 120,
            "track_id": "mic", "frame": 92, "processing_id": "plc-a",
            "parameters": {"fade_ms": 18}}]
        }"#;
        let manifest = parse_manifest(&resolved(raw), None).unwrap();
        assert_eq!(manifest.schema.detected_format, "new");
        assert_eq!(
            manifest.audio_path,
            Path::new("/tmp/case/recovered_replay.rplx")
        );
        assert_eq!(manifest.tracks[0].index, 2);
        assert_eq!(manifest.events[0].start_ms, 1850.0);
        assert_eq!(manifest.events[0].frame, Some(92));
        assert_eq!(
            manifest.events[0].effective_parameters["fade_ms"],
            json!(18)
        );
        assert_eq!(
            manifest.events[0].effective_parameters["algorithm"],
            json!("wsola")
        );
    }

    #[test]
    fn parses_legacy_seconds_track_and_timestamp() {
        let raw = r#"{
          "version": 1, "sample_rate": 8000, "channels": 1,
          "audio_path": "legacy.wav",
          "events": [
            {"t": 1.25, "duration": 0.08, "track": 0, "frame_index": "10",
             "plc_params": {"mode": "repeat"}},
            {"t": "00:00:02.500", "lost": true, "track": 0}
          ]
        }"#;
        let manifest = parse_manifest(&resolved(raw), None).unwrap();
        assert_eq!(manifest.schema.detected_format, "legacy");
        assert_eq!(manifest.events[0].start_ms, 1250.0);
        assert_eq!(manifest.events[0].duration_ms, Some(80.0));
        assert_eq!(manifest.events[1].start_ms, 2500.0);
        assert_eq!(manifest.events[1].kind, "packet_loss");
        assert_eq!(
            manifest.events[0].effective_parameters["mode"],
            json!("repeat")
        );
    }

    #[test]
    fn accepts_bare_event_array() {
        let raw = r#"[{"time": 0.5, "track": 0}, {"time_ms": 750, "track": 0}]"#;
        let manifest =
            parse_manifest(&resolved(raw), Some(Path::new("/tmp/case/unknown.rplx"))).unwrap();
        assert_eq!(manifest.schema.input_version, "array-v0");
        assert_eq!(manifest.events.len(), 2);
        assert_eq!(manifest.events[0].start_ms, 500.0);
        assert_eq!(manifest.events[1].start_ms, 750.0);
    }
}
