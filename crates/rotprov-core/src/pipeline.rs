use crate::gstprobe;
use crate::hash::fnv1a_hex;
use crate::model::*;
use crate::workspace::TempWorkspace;
use std::path::Path;

/// 处理记录最多保留的步骤数；超出即截断。
pub const MAX_KEPT_STEPS: usize = 32;
/// 截断时保留的头部步骤数（其余位置放截断标记与末步）。
const KEEP_HEAD: usize = MAX_KEPT_STEPS - 2;

struct StepBuilder {
    steps: Vec<ProcessingStep>,
}

impl StepBuilder {
    fn new() -> Self {
        Self { steps: Vec::new() }
    }

    fn push(&mut self, name: &str, detail: String, evidence_ids: Vec<String>) {
        let seq = self.steps.len() as u32 + 1;
        self.steps.push(ProcessingStep {
            seq,
            name: name.to_string(),
            detail,
            evidence_ids,
        });
    }
}

fn finalize_record(object_id: &str, steps: Vec<ProcessingStep>) -> ProcessRecord {
    let original_step_count = steps.len();
    if original_step_count <= MAX_KEPT_STEPS {
        return ProcessRecord {
            object_id: object_id.to_string(),
            steps,
            truncated: false,
            original_step_count,
            original_hash: None,
        };
    }
    // 截断：保留哈希与原始长度，使截断可核验。
    let full_json = serde_json::to_string(&steps).unwrap_or_default();
    let original_hash = Some(fnv1a_hex(&full_json));
    let mut kept: Vec<ProcessingStep> = steps[..KEEP_HEAD].to_vec();
    kept.push(ProcessingStep {
        seq: KEEP_HEAD as u32 + 1,
        name: "…[记录截断]…".to_string(),
        detail: format!(
            "省略 {} 步；原始 {} 步，哈希 {}",
            original_step_count - KEEP_HEAD - 1,
            original_step_count,
            original_hash.as_deref().unwrap_or("?")
        ),
        evidence_ids: Vec::new(),
    });
    if let Some(last) = steps.last() {
        kept.push(last.clone());
    }
    ProcessRecord {
        object_id: object_id.to_string(),
        steps: kept,
        truncated: true,
        original_step_count,
        original_hash,
    }
}

fn normalize_matrix(degrees: i64) -> Option<Rotation> {
    match degrees.rem_euclid(360) {
        0 => Some(Rotation::R0),
        90 => Some(Rotation::R90),
        180 => Some(Rotation::R180),
        270 => Some(Rotation::R270),
        _ => None,
    }
}

/// 处理单个对象；任何失败都转化为 Failed 结论，绝不向上抛出。
fn process_object(manifest: &InputManifest, ws: &TempWorkspace) -> ProvenanceEntry {
    let mut evidence: Vec<Evidence> = Vec::new();
    let mut snapshots: Vec<Snapshot> = Vec::new();
    let mut steps = StepBuilder::new();
    let mut state = ObjectState::default();

    let mut add_evidence = |kind: EvidenceKind, summary: String, content: String| {
        let id = format!("E{}-{:02}", manifest.id, evidence.len() + 1);
        evidence.push(Evidence {
            id: id.clone(),
            kind,
            summary,
            content,
        });
        id
    };

    let conclusion = match ingest(manifest, &mut state, &mut steps, &mut add_evidence) {
        Ok(()) => {
            snapshots.push(Snapshot {
                version: 1,
                stage: "ingest".to_string(),
                state: state.clone(),
            });
            normalize(manifest, &mut state, &mut steps, &mut add_evidence, ws);
            snapshots.push(Snapshot {
                version: 2,
                stage: "normalize".to_string(),
                state: state.clone(),
            });
            decide(&state, &mut steps, &mut add_evidence)
        }
        Err(err) => {
            let eid = add_evidence(
                EvidenceKind::ErrorContext,
                "异常现场".to_string(),
                err.clone(),
            );
            steps.push("error", format!("处理异常，已记录现场: {err}"), vec![eid]);
            Conclusion::Failed { error: err }
        }
    };

    ProvenanceEntry {
        manifest: manifest.clone(),
        conclusion,
        record: finalize_record(&manifest.id, steps.steps),
        snapshots,
        evidence,
    }
}

type EvidenceSink<'a> = dyn FnMut(EvidenceKind, String, String) -> String + 'a;

fn ingest(
    manifest: &InputManifest,
    state: &mut ObjectState,
    steps: &mut StepBuilder,
    add_evidence: &mut EvidenceSink,
) -> Result<(), String> {
    let sidecar_path = Path::new(&manifest.sidecar_path);
    let raw_text = std::fs::read_to_string(sidecar_path)
        .map_err(|e| format!("读取 sidecar 失败 {}: {e}", sidecar_path.display()))?;
    let raw: RawRotationInfo = serde_json::from_str(&raw_text)
        .map_err(|e| format!("解析 sidecar JSON 失败 {}: {e}", sidecar_path.display()))?;
    let eid = add_evidence(
        EvidenceKind::RawInput,
        format!("原始 sidecar（{} 字节）", raw_text.len()),
        raw_text.clone(),
    );
    steps.push(
        "ingest",
        format!("读取原始输入 {}", sidecar_path.display()),
        vec![eid],
    );

    // GStreamer 容器探测（真实管线：filesrc ! typefind ! fakesink）。
    let media_path = Path::new(&manifest.media_path);
    match gstprobe::probe_container(media_path) {
        Ok(caps) => {
            let eid = add_evidence(
                EvidenceKind::GstProbe,
                format!("容器探测: {caps}"),
                caps.clone(),
            );
            steps.push(
                "gst-probe",
                format!("typefind 识别 {} → {caps}", media_path.display()),
                vec![eid],
            );
            state.container_type = Some(caps);
        }
        Err(err) => {
            steps.push(
                "gst-probe",
                format!("容器探测未识别（不阻断）: {err}"),
                Vec::new(),
            );
        }
    }

    // 逐帧扫描日志（可能很长，触发记录截断）。
    #[derive(serde::Deserialize)]
    struct ScanLog {
        scan_log: Option<Vec<String>>,
    }
    if let Ok(extra) = serde_json::from_str::<ScanLog>(&raw_text) {
        if let Some(lines) = extra.scan_log {
            for (i, line) in lines.iter().enumerate() {
                steps.push("frame-scan", format!("帧 {i:04}: {line}"), Vec::new());
            }
        }
    }

    state.raw = Some(raw);
    Ok(())
}

fn normalize(
    manifest: &InputManifest,
    state: &mut ObjectState,
    steps: &mut StepBuilder,
    add_evidence: &mut EvidenceSink,
    ws: &TempWorkspace,
) {
    let Some(raw) = state.raw.clone() else { return };

    if let Some(tag) = &raw.orientation_tag {
        match gstprobe::normalize_orientation_tag(tag) {
            Some(rotation) => {
                let eid = add_evidence(
                    EvidenceKind::GstTagList,
                    format!("TagList 规范化: {tag} → {}", rotation.to_orientation_tag()),
                    format!(
                        "image-orientation={} ({}°)",
                        rotation.to_orientation_tag(),
                        rotation.degrees()
                    ),
                );
                steps.push(
                    "normalize-tag",
                    format!("GStreamer TagList 规范化 {tag} → {rotation}"),
                    vec![eid],
                );
                state.normalized_rotation = Some(rotation);
            }
            None => steps.push(
                "normalize-tag",
                format!("无法识别的方向标签 {tag:?}，按缺失处理"),
                Vec::new(),
            ),
        }
    }

    if let Some(degrees) = raw.display_matrix_degrees {
        match normalize_matrix(degrees) {
            Some(rotation) => {
                steps.push(
                    "normalize-matrix",
                    format!("显示矩阵 {degrees}° → {rotation}"),
                    Vec::new(),
                );
                state.matrix_rotation = Some(rotation);
            }
            None => steps.push(
                "normalize-matrix",
                format!("显示矩阵 {degrees}° 非 90 的倍数，按缺失处理"),
                Vec::new(),
            ),
        }
    }

    // 中间产物写入临时工作区（运行结束时统一清理）。
    let dump = serde_json::to_string_pretty(&state).unwrap_or_default();
    match ws.stage(&format!("normalized-{}.json", manifest.id), &dump) {
        Ok(path) => steps.push(
            "stage-temp",
            format!("规范化中间产物暂存 {}", path.display()),
            Vec::new(),
        ),
        Err(e) => steps.push("stage-temp", format!("暂存失败（不阻断）: {e}"), Vec::new()),
    }
}

fn decide(
    state: &ObjectState,
    steps: &mut StepBuilder,
    add_evidence: &mut EvidenceSink,
) -> Conclusion {
    let tag = state.normalized_rotation;
    let matrix = state.matrix_rotation;
    match (tag, matrix) {
        (Some(t), Some(m)) if t != m => {
            // 无法判断的边界：两条元数据互相矛盾，保留双方原值作为关键证据。
            let raw = state.raw.clone().unwrap_or_default();
            let eid = add_evidence(
                EvidenceKind::ConflictingValues,
                format!("冲突: 标签 {t} ↔ 矩阵 {m}"),
                format!(
                    "orientation_tag={:?}\ndisplay_matrix_degrees={:?}\ndevice_orientation={:?}",
                    raw.orientation_tag, raw.display_matrix_degrees, raw.device_orientation
                ),
            );
            steps.push(
                "decide",
                format!("标签与显示矩阵冲突（{t} ↔ {m}），无法判断，保留双方证据"),
                vec![eid],
            );
            Conclusion::Undecidable {
                reason: format!("旋转标签 {t} 与显示矩阵 {m} 矛盾"),
            }
        }
        (Some(rotation), _) | (None, Some(rotation)) => {
            if rotation == Rotation::R0 {
                steps.push("decide", "方向为 0°，无需修复".to_string(), Vec::new());
                Conclusion::NoFixNeeded
            } else {
                steps.push(
                    "decide",
                    format!("归一化旋转 {rotation} → 重写元数据为 0°"),
                    Vec::new(),
                );
                Conclusion::Fixed { from: rotation }
            }
        }
        (None, None) => {
            steps.push("decide", "无旋转元数据，无需修复".to_string(), Vec::new());
            Conclusion::NoFixNeeded
        }
    }
}

/// 运行整批处理：异常对象记为 Failed 后继续其余对象；
/// 结束时清理临时工作区并在报告中核验。
pub fn run(manifests: &[InputManifest]) -> RunReport {
    gstprobe::ensure_init().ok();
    let ws = match TempWorkspace::create() {
        Ok(ws) => ws,
        Err(e) => {
            return RunReport {
                entries: manifests
                    .iter()
                    .map(|m| ProvenanceEntry {
                        manifest: m.clone(),
                        conclusion: Conclusion::Failed {
                            error: format!("创建临时工作区失败: {e}"),
                        },
                        record: finalize_record(&m.id, Vec::new()),
                        snapshots: Vec::new(),
                        evidence: Vec::new(),
                    })
                    .collect(),
                temp_dir: String::new(),
                temp_cleaned: true,
            }
        }
    };
    let temp_dir = ws.path().to_string_lossy().to_string();

    let mut entries = Vec::new();
    for manifest in manifests {
        // 单对象异常不阻断批处理。
        let entry = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_object(manifest, &ws)
        }))
        .unwrap_or_else(|_| ProvenanceEntry {
            manifest: manifest.clone(),
            conclusion: Conclusion::Failed {
                error: "处理过程中发生 panic".to_string(),
            },
            record: finalize_record(&manifest.id, Vec::new()),
            snapshots: Vec::new(),
            evidence: Vec::new(),
        });
        entries.push(entry);
    }

    drop(ws); // 清理临时状态
    let temp_cleaned = !Path::new(&temp_dir).exists();
    RunReport {
        entries,
        temp_dir,
        temp_cleaned,
    }
}
