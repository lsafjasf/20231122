use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::{
    is_known_media_extension, sane_fps, Checkpoint, MediaObject, Project, ProjectFormat,
    TypesetParameters,
};
use crate::timecode::{parse_time, parse_time_str, DEFAULT_FPS};
use crate::{CoreError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSource {
    pub path: PathBuf,
    pub format: ProjectFormat,
    pub extension: Option<String>,
    pub unknown_extension: bool,
    pub size_bytes: u64,
    pub sha256: String,
    pub magic_hex: String,
}

#[derive(Debug, Clone)]
pub struct LoadedProject {
    pub project: Project,
    pub source: ProjectSource,
}

pub fn load_project(path: &Path) -> Result<LoadedProject> {
    let bytes = std::fs::read(path).map_err(|source| CoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let extension = extension_of(path);
    let mut source = ProjectSource {
        path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
        format: ProjectFormat::BareMedia,
        extension: extension.clone(),
        unknown_extension: extension
            .as_deref()
            .map(|ext| !is_known_media_extension(ext))
            .unwrap_or(true),
        size_bytes: bytes.len() as u64,
        sha256: sha256_bytes(&bytes),
        magic_hex: hex::encode(&bytes[..bytes.len().min(64)]),
    };

    if !looks_like_json(&bytes) {
        let project = project_from_bare_media(path, extension.as_deref());
        return Ok(LoadedProject {
            source: ProjectSource {
                format: ProjectFormat::BareMedia,
                ..source
            },
            project,
        });
    }

    let value: Value = serde_json::from_slice(&bytes)?;
    let format = detect_format(&value)?;
    let mut project = match format {
        ProjectFormat::RollV2 => parse_v2(&value, path)?,
        ProjectFormat::LegacyV1 => parse_legacy(&value, path)?,
        ProjectFormat::BareMedia => unreachable!("JSON is never treated as bare media"),
    };
    normalize_project(&mut project, path);
    source.unknown_extension = false;
    Ok(LoadedProject {
        project,
        source: ProjectSource { format, ..source },
    })
}

fn looks_like_json(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .is_some_and(|byte| *byte == b'{' || *byte == b'[')
}

pub fn detect_format(value: &Value) -> Result<ProjectFormat> {
    let schema = value.get("schema").and_then(Value::as_str);
    if schema.is_some_and(|schema| schema.starts_with("credits-roll/v2")) {
        return Ok(ProjectFormat::RollV2);
    }
    if value.get("timeline").is_some() || value.get("tracks").is_some() {
        return Ok(ProjectFormat::RollV2);
    }
    if value.get("items").is_some()
        || value.get("clips").is_some()
        || value.get("objects").is_some()
        || value.get("format").and_then(Value::as_str) == Some("legacy")
    {
        return Ok(ProjectFormat::LegacyV1);
    }
    if value.get("version").is_some() {
        return Ok(
            if value
                .get("version")
                .and_then(Value::as_u64)
                .is_some_and(|version| version >= 2)
            {
                ProjectFormat::RollV2
            } else {
                ProjectFormat::LegacyV1
            },
        );
    }
    Err(CoreError::UnsupportedSchema(
        schema
            .unwrap_or("missing schema and legacy item list")
            .to_string(),
    ))
}

fn parse_v2(value: &Value, path: &Path) -> Result<Project> {
    let timeline = value.get("timeline").unwrap_or(value);
    let fps = timeline
        .get("fps")
        .or_else(|| value.get("fps"))
        .and_then(Value::as_f64)
        .map(sane_fps)
        .unwrap_or(DEFAULT_FPS);
    let mut project = Project::new(
        ProjectFormat::RollV2,
        value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled credits roll"),
        fps,
    );
    project.defaults = parse_typeset(
        value
            .get("typeset")
            .or_else(|| value.get("defaults"))
            .or_else(|| timeline.get("typeset")),
        &TypesetParameters::default(),
    )
    .normalized();

    let global_points = timeline
        .get("review_points")
        .or_else(|| value.get("review_points"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let tracks = timeline
        .get("tracks")
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::InvalidProject("v2 timeline has no tracks[]".to_string()))?;

    for (track_index, track) in tracks.iter().enumerate() {
        let track_id = track
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("track-{track_index}"));
        let kind = track
            .get("kind")
            .or_else(|| track.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("video");
        let objects = track
            .get("objects")
            .or_else(|| track.get("items"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for (object_index, object) in objects.iter().enumerate() {
            let Some(media_path) = object
                .get("src")
                .or_else(|| object.get("path"))
                .or_else(|| object.get("file"))
                .and_then(Value::as_str)
            else {
                if kind == "credits" || object.get("text").is_some() {
                    project.warnings.push(format!(
                        "track {track_id}: inline credits text is used as the default overlay text"
                    ));
                } else {
                    project.warnings.push(format!(
                        "track {track_id}: object {object_index} has no src/path/file; skipped"
                    ));
                }
                continue;
            };
            let mut checkpoints = parse_checkpoints(
                object
                    .get("review_points")
                    .or_else(|| object.get("checkpoints"))
                    .or_else(|| object.get("snapshots")),
                fps,
            )?;
            if checkpoints.is_empty() {
                checkpoints = parse_checkpoints(Some(&Value::Array(global_points.clone())), fps)?;
            }
            let start_s = parse_optional_time(
                object
                    .get("in")
                    .or_else(|| object.get("start"))
                    .or_else(|| object.get("begin")),
            )?
            .unwrap_or(0.0);
            let end_s = parse_optional_time(
                object
                    .get("out")
                    .or_else(|| object.get("end"))
                    .or_else(|| object.get("finish")),
            )?;
            let duration_s = object
                .get("duration")
                .and_then(Value::as_f64)
                .or_else(|| end_s.map(|end| (end - start_s).max(0.0)));
            project.objects.push(MediaObject {
                id: object
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{track_id}-{object_index}")),
                path: resolve_media_path(path, media_path),
                track: object
                    .get("track")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("{track_id}:video")),
                start_s,
                duration_s,
                checkpoints,
                parameters: parse_typeset(
                    object.get("typeset").or_else(|| object.get("parameters")),
                    &project.defaults,
                )
                .normalized(),
                credits_text: object
                    .get("text")
                    .or_else(|| object.get("credits"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or_else(|| {
                        value
                            .get("credits")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    }),
                source_note: object
                    .get("note")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
    }
    Ok(project)
}

fn parse_legacy(value: &Value, path: &Path) -> Result<Project> {
    let fps = value
        .get("fps")
        .or_else(|| value.get("frame_rate"))
        .and_then(Value::as_f64)
        .map(sane_fps)
        .unwrap_or(DEFAULT_FPS);
    let mut project = Project::new(
        ProjectFormat::LegacyV1,
        value
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Legacy credits roll"),
        fps,
    );
    project.defaults = parse_typeset(Some(value), &TypesetParameters::default()).normalized();

    let items = value
        .get("items")
        .or_else(|| value.get("clips"))
        .or_else(|| value.get("objects"))
        .and_then(Value::as_array)
        .ok_or_else(|| CoreError::InvalidProject("legacy project has no items[]".to_string()))?;

    for (index, item) in items.iter().enumerate() {
        let Some(media_path) = item
            .get("file")
            .or_else(|| item.get("path"))
            .or_else(|| item.get("src"))
            .and_then(Value::as_str)
        else {
            project
                .warnings
                .push(format!("legacy item {index} has no file path; skipped"));
            continue;
        };
        let start_s = parse_optional_time(
            item.get("start")
                .or_else(|| item.get("in"))
                .or_else(|| item.get("begin")),
        )?
        .unwrap_or(0.0);
        let end_s = parse_optional_time(
            item.get("end")
                .or_else(|| item.get("out"))
                .or_else(|| item.get("finish")),
        )?;
        let duration_s = item
            .get("duration")
            .and_then(Value::as_f64)
            .or_else(|| end_s.map(|end| (end - start_s).max(0.0)));
        let mut checkpoints = parse_checkpoints(
            item.get("snapshots")
                .or_else(|| item.get("review_points"))
                .or_else(|| item.get("checkpoints"))
                .or_else(|| item.get("snapshot")),
            fps,
        )?;
        if checkpoints.is_empty() {
            if let Some(snapshot) = item.get("snapshot_at") {
                checkpoints.push(Checkpoint {
                    seconds: parse_time(snapshot)?,
                    requested: Some(snapshot.to_string()),
                    label: Some("legacy snapshot".to_string()),
                    track: None,
                    requested_frame: None,
                });
            }
        }
        project.objects.push(MediaObject {
            id: item
                .get("id")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("legacy-{index}")),
            path: resolve_media_path(path, media_path),
            track: parse_track(item.get("track")),
            start_s,
            duration_s,
            checkpoints,
            parameters: parse_typeset(Some(item), &project.defaults).normalized(),
            credits_text: item
                .get("text")
                .or_else(|| item.get("credits"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    value
                        .get("credits")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                }),
            source_note: item.get("note").and_then(Value::as_str).map(str::to_string),
        });
    }
    Ok(project)
}

fn project_from_bare_media(path: &Path, extension: Option<&str>) -> Project {
    let fps = DEFAULT_FPS;
    let mut project = Project::new(
        ProjectFormat::BareMedia,
        path.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown media"),
        fps,
    );
    project.warnings.push(format!(
        "input was not JSON; treated as a single media object (extension: {})",
        extension.unwrap_or("<none>")
    ));
    project.objects.push(MediaObject {
        id: sanitize_id(
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("media"),
        ),
        path: path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
        track: "video:0".to_string(),
        start_s: 0.0,
        duration_s: None,
        checkpoints: Vec::new(),
        parameters: project.defaults.clone(),
        credits_text: None,
        source_note: Some("implicit bare-media object".to_string()),
    });
    project
}

fn normalize_project(project: &mut Project, path: &Path) {
    project.fps = sane_fps(project.fps);
    let mut seen = HashSet::new();
    for (index, object) in project.objects.iter_mut().enumerate() {
        object.id = sanitize_id(&object.id);
        if object.id.is_empty() {
            object.id = format!("object-{index}");
        }
        if !seen.insert(object.id.clone()) {
            let mut suffix = 2;
            let base = object.id.clone();
            while !seen.insert(format!("{base}-{suffix}")) {
                suffix += 1;
            }
            object.id = format!("{base}-{suffix}");
        }
        object.parameters = object.parameters.clone().normalized();
        if object.track.trim().is_empty() {
            object.track = "video:0".to_string();
        }
        for checkpoint in &mut object.checkpoints {
            checkpoint.seconds = checkpoint.seconds.max(0.0);
            if checkpoint.track.is_none() {
                checkpoint.track = Some(object.track.clone());
            }
        }
    }
    if project.objects.is_empty() {
        project.warnings.push(format!(
            "{} contains no media objects",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("project")
        ));
    }
}

fn parse_checkpoints(value: Option<&Value>, fps: f64) -> Result<Vec<Checkpoint>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| parse_checkpoint(item, fps))
            .collect(),
        other => Ok(vec![parse_checkpoint(other, fps)?]),
    }
}

fn parse_checkpoint(value: &Value, fps: f64) -> Result<Checkpoint> {
    match value {
        Value::Object(map) => {
            let requested_frame = map.get("frame").and_then(Value::as_u64);
            let at_value = map
                .get("at")
                .or_else(|| map.get("time"))
                .or_else(|| map.get("seconds"));
            let mut seconds = match at_value {
                Some(value) => parse_time(value)?,
                None => requested_frame
                    .map(|frame| frame as f64 / fps.max(1.0))
                    .unwrap_or(0.0),
            };
            if let Some(frame) = requested_frame {
                seconds = frame as f64 / fps.max(1.0);
            }
            Ok(Checkpoint {
                seconds,
                requested: at_value.map(|value| value.to_string().trim_matches('"').to_string()),
                label: map.get("label").and_then(Value::as_str).map(str::to_string),
                track: map.get("track").and_then(Value::as_str).map(str::to_string),
                requested_frame,
            })
        }
        other => Ok(Checkpoint {
            seconds: parse_time(other)?,
            requested: Some(other.to_string().trim_matches('"').to_string()),
            label: None,
            track: None,
            requested_frame: None,
        }),
    }
}

fn parse_optional_time(value: Option<&Value>) -> Result<Option<f64>> {
    value.map(parse_time).transpose()
}

fn parse_track(value: Option<&Value>) -> String {
    match value {
        Some(Value::Number(number)) => format!("video:{number}"),
        Some(Value::String(track)) => track.clone(),
        _ => "video:0".to_string(),
    }
}

fn parse_typeset(value: Option<&Value>, base: &TypesetParameters) -> TypesetParameters {
    let Some(value) = value else {
        return base.clone();
    };
    let mut parameters = base.clone();
    if let Some(font) = value
        .get("font")
        .or_else(|| value.get("font_desc"))
        .and_then(Value::as_str)
    {
        let (family, size) = parse_font_desc(font);
        parameters.font_family = family;
        if let Some(size) = size {
            parameters.font_size = size;
        }
    }
    if let Some(family) = value.get("font_family").and_then(Value::as_str) {
        parameters.font_family = family.to_string();
    }
    if let Some(size) = value.get("font_size").and_then(Value::as_f64) {
        parameters.font_size = size;
    }
    if let Some(color) = value.get("color").and_then(Value::as_str) {
        parameters.color = color.to_string();
    }
    if let Some(shadow) = value.get("shadow").and_then(Value::as_bool) {
        parameters.shadow = shadow;
    }
    if let Some(speed) = value
        .get("scroll_speed_px_s")
        .or_else(|| value.get("speed"))
        .and_then(Value::as_f64)
    {
        parameters.scroll_speed_px_s = speed;
    }
    if let Some(start) = value
        .get("roll_start_s")
        .or_else(|| value.get("roll_start"))
        .and_then(Value::as_f64)
    {
        parameters.roll_start_s = start;
    }
    if let Some(margin) = value.get("horizontal_margin").and_then(Value::as_f64) {
        parameters.horizontal_margin = if margin > 1.0 {
            (margin / 1920.0).clamp(0.0, 0.45)
        } else {
            margin
        };
    }
    if let Some(spacing) = value.get("line_spacing").and_then(Value::as_f64) {
        parameters.line_spacing = spacing;
    }
    if let Some(opacity) = value.get("opacity").and_then(Value::as_f64) {
        parameters.opacity = opacity;
    }
    parameters
}

fn parse_font_desc(raw: &str) -> (String, Option<f64>) {
    let trimmed = raw.trim();
    if let Some((family, size)) = trimmed.rsplit_once(' ') {
        if let Ok(size) = size.parse::<f64>() {
            return (family.trim().to_string(), Some(size));
        }
    }
    (trimmed.to_string(), None)
}

fn resolve_media_path(project: &Path, media: &str) -> PathBuf {
    let media_path = Path::new(media);
    if media_path.is_absolute() {
        return media_path.to_path_buf();
    }
    let joined = project
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(media_path);
    joined.canonicalize().unwrap_or(joined)
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}

fn sanitize_id(raw: &str) -> String {
    let mut id = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            id.push(character.to_ascii_lowercase());
        } else if character.is_whitespace() {
            id.push('-');
        }
    }
    while id.contains("--") {
        id = id.replace("--", "-");
    }
    id.trim_matches('-').to_string()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn parse_time_literal(raw: &str) -> Result<f64> {
    parse_time_str(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_legacy_and_v2() {
        assert_eq!(
            detect_format(&json!({"items": []})).unwrap(),
            ProjectFormat::LegacyV1
        );
        assert_eq!(
            detect_format(&json!({"schema": "credits-roll/v2", "timeline": {}})).unwrap(),
            ProjectFormat::RollV2
        );
        assert_eq!(
            detect_format(&json!({"version": 2, "items": []})).unwrap(),
            ProjectFormat::LegacyV1
        );
    }

    #[test]
    fn parses_legacy_font_and_time() {
        let (family, size) = parse_font_desc("Sans Bold 28");
        assert_eq!(family, "Sans Bold");
        assert_eq!(size, Some(28.0));
        assert_eq!(parse_time_literal("00:00:02.5").unwrap(), 2.5);
    }

    #[test]
    fn sanitizes_ids() {
        assert_eq!(sanitize_id("Roll 01 / A"), "roll-01-a");
    }
}
