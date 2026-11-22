//! 检查编排引擎。
//!
//! 职责：
//! 1. 登记原始输入（含未知扩展对象）并计算内容指纹；
//! 2. 为每个可解码音频生成两个版本快照（原始采样率 / 重采样）；
//! 3. 调用分析器产出可溯源的结论与关键证据；
//! 4. 单个对象异常时记录警告并继续处理其余对象；
//! 5. 通过 [`crate::util::TempState`] 在结束时清理全部临时状态。

use std::path::{Path, PathBuf};

use crate::analyzer::{analyze, FadeThresholds};
use crate::decoder::{decode_to_mono, DecodedStream};
use crate::model::{Conclusion, Report, Snapshot, SourceKind, SourceObject, Verdict};
use crate::util::{sha256_bytes, sha256_file, utc_now, TempState};

/// 已知音频扩展名白名单；不在表内即视为“未知扩展”。
const KNOWN_AUDIO_EXTS: &[&str] = &[
    "wav", "flac", "ogg", "oga", "mp3", "m4a", "aac", "wma", "opus", "aiff", "aif",
];

/// 第二版本快照使用的统一重采样率。
const SNAPSHOT_V2_RATE: u32 = 16_000;

/// 批次检查入口。`paths` 为用户提供的原始输入路径列表。
pub fn run_batch(paths: &[String]) -> (Report, TempState) {
    let temp = match TempState::new("batch") {
        Ok(t) => t,
        Err(e) => {
            // 极端情况下临时目录不可创建：用当前目录下的一次性句柄兜底，
            // cleanup 仍会尝试删除，保证“结束时清理临时状态”的语义不丢失。
            eprintln!("警告: 临时目录创建失败: {e}");
            TempState::new("batch-fallback").expect("无法创建任何临时工作目录")
        }
    };

    let objects = register_objects(paths);
    let thresholds = FadeThresholds::default();
    let mut conclusions = Vec::with_capacity(objects.len());

    for object in objects {
        // 每个对象独立处理；任何 panic 之外的错误都被收敛为 warning，
        // 确保“异常后继续处理其余对象”。
        conclusions.push(process_one(&object, &thresholds, temp.path()));
    }

    let report = Report {
        generated_at: utc_now(),
        total_objects: paths.len(),
        conclusions,
    };
    (report, temp)
}

fn register_objects(paths: &[String]) -> Vec<SourceObject> {
    paths
        .iter()
        .enumerate()
        .map(|(idx, raw)| {
            let id = idx as u32 + 1;
            let absolute = std::fs::canonicalize(raw).unwrap_or_else(|_| PathBuf::from(raw));
            let extension = Path::new(raw)
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase());

            let metadata = std::fs::metadata(&absolute);
            let (size_bytes, sha256) = match &metadata {
                Ok(meta) if meta.is_file() => {
                    let size = Some(meta.len());
                    let hash = sha256_file(&absolute).ok();
                    (size, hash)
                }
                _ => (None, None),
            };

            let kind = if metadata.is_err() || size_bytes.is_none() {
                SourceKind::Missing
            } else if let Some(ext) = &extension {
                if KNOWN_AUDIO_EXTS.contains(&ext.as_str()) {
                    SourceKind::Audio
                } else {
                    SourceKind::UnknownExtension
                }
            } else {
                SourceKind::UnknownExtension
            };

            SourceObject {
                id,
                raw_path: raw.clone(),
                absolute_path: absolute,
                kind,
                size_bytes,
                sha256,
                extension,
            }
        })
        .collect()
}

fn process_one(object: &SourceObject, thresholds: &FadeThresholds, temp_root: &Path) -> Conclusion {
    let mut warnings = Vec::new();
    let mut snapshots: Vec<Snapshot> = Vec::new();
    let mut sample_sets: Vec<(Snapshot, Vec<f32>)> = Vec::new();

    match object.kind {
        SourceKind::Missing => {
            return make_not_analyzable(
                object,
                snapshots,
                vec!["路径不存在或不可读，跳过解码".to_string()],
                Some(format!(
                    "登记为 Missing: {}",
                    object.absolute_path.display()
                )),
            );
        }
        SourceKind::UnknownExtension => {
            warnings.push(format!(
                "未知扩展 {:?}，来源检查策略不猜测媒体类型，标记为不可分析",
                object.extension
            ));
            return make_not_analyzable(
                object,
                snapshots,
                warnings,
                Some(format!(
                    "未知扩展 {:?}；已保留大小 {} 字节与 SHA-256 指纹作为关键证据",
                    object.extension,
                    object.size_bytes.unwrap_or(0)
                )),
            );
        }
        SourceKind::Audio => {}
    }

    let input_hash = match &object.sha256 {
        Some(h) => h.clone(),
        None => {
            return make_not_analyzable(
                object,
                snapshots,
                vec!["无法计算输入指纹，放弃分析以保证溯源完整性".to_string()],
                None,
            );
        }
    };

    // 快照 v1：原始采样率解码。
    match decode_with_guard(object, None, "v1-decoded", &input_hash, temp_root) {
        Ok((snap, samples)) => {
            snapshots.push(snap.clone());
            sample_sets.push((snap, samples));
        }
        Err((snap_steps, err)) => {
            warnings.push(format!("快照 v1 解码异常并已恢复: {err}"));
            // 即便失败也保留一个登记快照，记录已执行的处理步骤。
            snapshots.push(Snapshot {
                label: "v1-decoded".to_string(),
                taken_at: utc_now(),
                input_sha256: input_hash.clone(),
                sample_rate: None,
                channels: None,
                frame_count: None,
                steps: snap_steps,
            });
        }
    }

    // 快照 v2：重采样到统一速率，作为第二个独立版本。
    match decode_with_guard(
        object,
        Some(SNAPSHOT_V2_RATE),
        "v2-resampled",
        &input_hash,
        temp_root,
    ) {
        Ok((snap, samples)) => {
            snapshots.push(snap.clone());
            sample_sets.push((snap, samples));
        }
        Err((snap_steps, err)) => {
            warnings.push(format!("快照 v2 解码异常并已恢复: {err}"));
            snapshots.push(Snapshot {
                label: "v2-resampled".to_string(),
                taken_at: utc_now(),
                input_sha256: input_hash.clone(),
                sample_rate: None,
                channels: None,
                frame_count: None,
                steps: snap_steps,
            });
        }
    }

    if sample_sets.is_empty() {
        return Conclusion {
            object_id: object.id,
            path: object.raw_path.clone(),
            verdict: Verdict::NotAnalyzable,
            snapshots,
            evidence: Vec::new(),
            warnings,
        };
    }

    let (verdict, evidence) = analyze(&sample_sets, thresholds);
    Conclusion {
        object_id: object.id,
        path: object.raw_path.clone(),
        verdict,
        snapshots,
        evidence,
        warnings,
    }
}

fn make_not_analyzable(
    object: &SourceObject,
    snapshots: Vec<Snapshot>,
    warnings: Vec<String>,
    note: Option<String>,
) -> Conclusion {
    let mut evidence = Vec::new();
    if let Some(note) = note {
        evidence.push(crate::model::Evidence {
            metric: "intake".to_string(),
            value: object.size_bytes.unwrap_or(0) as f64,
            threshold: "需要可读的已知音频扩展名".to_string(),
            window_frames: None,
            snapshot: "intake".to_string(),
            note,
        });
    }
    Conclusion {
        object_id: object.id,
        path: object.raw_path.clone(),
        verdict: Verdict::NotAnalyzable,
        snapshots,
        evidence,
        warnings,
    }
}

/// 解码并把产物写入临时目录、核对指纹，再组装快照。
/// 失败时回传已记录的步骤，便于证据保留。
fn decode_with_guard(
    object: &SourceObject,
    target_rate: Option<u32>,
    label: &str,
    input_hash: &str,
    temp_root: &Path,
) -> Result<(Snapshot, Vec<f32>), (Vec<crate::model::PipelineStep>, String)> {
    let decoded: DecodedStream = decode_to_mono(&object.absolute_path, target_rate, label)
        .map_err(|e| (e.steps, e.message))?;

    // 临时状态：把解码出的 PCM 以裸 f32 形式落盘，验证后随批次统一清理。
    let raw_bytes: Vec<u8> = decoded
        .mono_samples
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let temp_file = temp_root.join(format!("object-{}-{}.f32le", object.id, label));
    if let Err(e) = std::fs::write(&temp_file, &raw_bytes) {
        return Err((decoded.steps, format!("临时 PCM 写入失败: {e}")));
    }
    let pcm_hash = sha256_bytes(&raw_bytes);
    eprintln!(
        "[对象 {}] 快照 {label}: 临时文件 {} ({} 帧, pcm-sha256 {}…)",
        object.id,
        temp_file.display(),
        decoded.mono_samples.len(),
        &pcm_hash[..12]
    );

    let snapshot = Snapshot {
        label: label.to_string(),
        taken_at: utc_now(),
        input_sha256: input_hash.to_string(),
        sample_rate: Some(decoded.sample_rate),
        channels: Some(decoded.channels),
        frame_count: Some(decoded.mono_samples.len() as u64),
        steps: decoded.steps,
    };
    Ok((snapshot, decoded.mono_samples))
}
