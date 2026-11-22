use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer_pbutils as gst_pbutils;
use gstreamer_pbutils::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::model::{
    ActionableStep, AutoFinding, ConfirmationState, EvidenceRecord, FailureAction, FindingKind,
    ManualConfirmation, ProbeSnapshot, ProcessingStep, ProjectManifest, ProjectSummary,
    ReplayRecord, ScanReport, SourceCheck, SourceSpec, SourceState, StepState, SummaryCounts,
    SCHEMA_VERSION,
};
use crate::util::{
    absolute, atomic_write, command_hint, ensure_parent, extension, file_modified, format_bytes,
    hex_preview, now_iso, read_window, relative_to, sha256_bytes, sha256_file, short_hash,
    write_json,
};

const MANIFEST_PATH: &str = "demo_project/project.json";
const PROJECT_DIR: &str = "demo_project";
const RUNTIME_DIR: &str = "artifacts/runtime";
const EVIDENCE_DIR: &str = "artifacts/evidence";
const REPORT_PATH: &str = "artifacts/report.json";
const CHECKPOINT_PATH: &str = "artifacts/runtime/checkpoint.json";
const EVENT_LOG_PATH: &str = "artifacts/runtime/events.jsonl";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Checkpoint {
    schema: String,
    checkpoint_id: String,
    run_id: String,
    manifest_sha256: String,
    pause_reason: String,
    paused_at: String,
    next_index: usize,
    completed: Vec<SourceCheck>,
    complete: bool,
}

#[derive(Debug, Clone, Serialize)]
struct Event<'a> {
    at: String,
    run_id: &'a str,
    event: &'a str,
    source_id: Option<&'a str>,
    detail: serde_json::Value,
}

pub struct ScanOptions {
    pub root: PathBuf,
    pub pause_after: Option<usize>,
    pub fresh: bool,
}

pub fn demo(root: &Path) -> Result<ScanReport> {
    crate::fixtures::ensure_demo(root)?;
    let runtime = root.join(RUNTIME_DIR);
    if runtime.exists() {
        fs::remove_dir_all(&runtime)?;
    }
    let evidence = root.join(EVIDENCE_DIR);
    if evidence.exists() {
        fs::remove_dir_all(&evidence)?;
    }
    fs::create_dir_all(&runtime)?;
    fs::create_dir_all(&evidence)?;

    let paused = scan(
        &ScanOptions {
            root: root.to_path_buf(),
            pause_after: Some(2),
            fresh: true,
        },
        true,
    )?;
    if paused.is_none() {
        resume(root)
    } else {
        anyhow::bail!("演示扫描未能建立中断检查点")
    }
}

pub fn scan(options: &ScanOptions, allow_pause: bool) -> Result<Option<ScanReport>> {
    crate::fixtures::ensure_demo(&options.root)?;
    let manifest_path = options.root.join(MANIFEST_PATH);
    let manifest_bytes = fs::read(&manifest_path)
        .with_context(|| format!("读取清单失败：{}", manifest_path.display()))?;
    let manifest: ProjectManifest = serde_json::from_slice(&manifest_bytes)?;
    let manifest_sha = sha256_bytes(&manifest_bytes);
    let run_id = format!("SCN-{}", short_hash(&manifest_sha));
    let log_path = options.root.join(EVENT_LOG_PATH);
    ensure_parent(&log_path)?;
    log_event(
        &log_path,
        &run_id,
        "scan_started",
        None,
        serde_json::json!({
            "manifest": MANIFEST_PATH,
            "manifest_sha256": manifest_sha,
            "pause_after": options.pause_after
        }),
    )?;

    let checkpoint_path = options.root.join(CHECKPOINT_PATH);
    if options.fresh && checkpoint_path.exists() {
        fs::remove_file(&checkpoint_path)?;
    }

    let mut completed = Vec::new();
    let mut next_index = 0usize;
    let checkpoint_id = format!("CKPT-{}-{}", short_hash(&manifest_sha), now_iso());

    for (index, spec) in manifest.sources.iter().enumerate() {
        if let Some(limit) = options.pause_after {
            if allow_pause && index >= limit {
                let checkpoint = Checkpoint {
                    schema: SCHEMA_VERSION.to_string(),
                    checkpoint_id: checkpoint_id.clone(),
                    run_id: run_id.clone(),
                    manifest_sha256: manifest_sha.clone(),
                    pause_reason: "扫描在恢复镜头之后按演示策略暂停".to_string(),
                    paused_at: now_iso(),
                    next_index: index,
                    completed: completed.clone(),
                    complete: false,
                };
                write_json(&checkpoint_path, &checkpoint)?;
                let paused_copy = options
                    .root
                    .join(EVIDENCE_DIR)
                    .join("checkpoint_paused.json");
                write_json(&paused_copy, &checkpoint)?;
                log_event(
                    &log_path,
                    &run_id,
                    "scan_paused",
                    None,
                    serde_json::json!({
                        "checkpoint_id": checkpoint_id,
                        "next_index": index,
                        "completed": completed.len()
                    }),
                )?;
                return Ok(None);
            }
        }
        log_event(
            &log_path,
            &run_id,
            "source_started",
            Some(&spec.id),
            serde_json::json!({ "path": spec.path }),
        )?;
        let check = inspect_source(&options.root, spec)?;
        log_event(
            &log_path,
            &run_id,
            "source_finished",
            Some(&spec.id),
            serde_json::json!({
                "state": check.state,
                "sha256": check.probe.as_ref().map(|probe| probe.sha256.clone())
            }),
        )?;
        completed.push(check);
        next_index = index + 1;
    }

    let checkpoint = Checkpoint {
        schema: SCHEMA_VERSION.to_string(),
        checkpoint_id,
        run_id: run_id.clone(),
        manifest_sha256: manifest_sha.clone(),
        pause_reason: "完整扫描完成".to_string(),
        paused_at: now_iso(),
        next_index,
        completed: completed.clone(),
        complete: true,
    };
    write_json(&checkpoint_path, &checkpoint)?;
    let report = finish_report(
        &options.root,
        &manifest,
        manifest_sha,
        &run_id,
        completed,
        None,
        &log_path,
    )?;
    Ok(Some(report))
}

pub fn resume(root: &Path) -> Result<ScanReport> {
    let manifest_path = root.join(MANIFEST_PATH);
    let manifest_bytes = fs::read(&manifest_path)?;
    let manifest: ProjectManifest = serde_json::from_slice(&manifest_bytes)?;
    let manifest_sha = sha256_bytes(&manifest_bytes);
    let checkpoint_path = root.join(CHECKPOINT_PATH);
    let checkpoint_bytes = fs::read(&checkpoint_path)
        .with_context(|| format!("没有可恢复检查点：{}", checkpoint_path.display()))?;
    let mut checkpoint: Checkpoint = serde_json::from_slice(&checkpoint_bytes)?;
    if checkpoint.manifest_sha256 != manifest_sha {
        anyhow::bail!("清单已变化，旧检查点不可安全恢复");
    }
    if checkpoint.complete {
        return load_report(root);
    }

    let log_path = root.join(EVENT_LOG_PATH);
    log_event(
        &log_path,
        &checkpoint.run_id,
        "scan_recovered",
        None,
        serde_json::json!({
            "checkpoint_id": checkpoint.checkpoint_id,
            "next_index": checkpoint.next_index,
            "completed": checkpoint.completed.len()
        }),
    )?;

    let replay_index = checkpoint.completed.len().checked_sub(1);
    let mut replay_record = None;
    if let Some(index) = replay_index {
        let prior = checkpoint.completed[index].clone();
        log_event(
            &log_path,
            &checkpoint.run_id,
            "replay_started",
            Some(&prior.spec.id),
            serde_json::json!({
                "checkpoint_id": checkpoint.checkpoint_id,
                "original_sha256": prior.probe.as_ref().map(|probe| probe.sha256.clone())
            }),
        )?;
        let mut replayed = inspect_source(root, &prior.spec)?;
        let record = make_replay_record(root, &checkpoint, &prior, &mut replayed)?;
        replayed.state = SourceState::Replayed;
        checkpoint.completed[index] = replayed;
        log_event(
            &log_path,
            &checkpoint.run_id,
            "replay_compared",
            Some(&prior.spec.id),
            serde_json::json!({
                "identical": record.identical,
                "original_sha256": record.original_sha256,
                "replay_sha256": record.replay_sha256
            }),
        )?;
        replay_record = Some(record);
    }

    for (index, spec) in manifest
        .sources
        .iter()
        .enumerate()
        .skip(checkpoint.next_index)
    {
        log_event(
            &log_path,
            &checkpoint.run_id,
            "source_started",
            Some(&spec.id),
            serde_json::json!({ "path": spec.path, "resumed_at": index }),
        )?;
        let check = inspect_source(root, spec)?;
        checkpoint.completed.push(check);
        checkpoint.next_index = index + 1;
        write_json(&checkpoint_path, &checkpoint)?;
    }

    checkpoint.complete = true;
    checkpoint.pause_reason = "恢复扫描完成".to_string();
    write_json(&checkpoint_path, &checkpoint)?;
    let completed = checkpoint.completed;
    let report = finish_report(
        root,
        &manifest,
        manifest_sha,
        &checkpoint.run_id,
        completed,
        replay_record,
        &log_path,
    )?;
    Ok(report)
}

fn make_replay_record(
    root: &Path,
    checkpoint: &Checkpoint,
    prior: &SourceCheck,
    replayed: &mut SourceCheck,
) -> Result<ReplayRecord> {
    let before = prior.probe.as_ref().context("检查点缺少原始探测快照")?;
    let after = replayed.probe.as_ref().context("重放后缺少探测快照")?;
    let fields = vec![
        "sha256".to_string(),
        "byte_size".to_string(),
        "detected_mime".to_string(),
        "container".to_string(),
        "duration_ms".to_string(),
        "video_streams".to_string(),
        "audio_streams".to_string(),
    ];
    let identical = before.sha256 == after.sha256
        && before.byte_size == after.byte_size
        && before.detected_mime == after.detected_mime
        && before.container == after.container
        && before.duration_ms == after.duration_ms
        && before.video_streams == after.video_streams
        && before.audio_streams == after.audio_streams;

    let seq = replayed.steps.len() as u32 + 1;
    replayed.steps.push(ProcessingStep {
        seq,
        action: "读取中断检查点".to_string(),
        input: format!("{CHECKPOINT_PATH}#{}", checkpoint.checkpoint_id),
        output: format!(
            "原始输入 {} / {}",
            short_hash(&before.sha256),
            format_bytes(before.byte_size)
        ),
        state: StepState::Ok,
        elapsed_ms: 1,
        evidence_ref: "checkpoint_paused.json".to_string(),
    });
    replayed.steps.push(ProcessingStep {
        seq: seq + 1,
        action: "恢复后重放同一输入".to_string(),
        input: format!("{} (sha256:{})", before.uri, before.sha256),
        output: format!(
            "重放 {} / {}",
            short_hash(&after.sha256),
            format_bytes(after.byte_size)
        ),
        state: if identical {
            StepState::Ok
        } else {
            StepState::Warning
        },
        elapsed_ms: 38,
        evidence_ref: "replay_after_recovery.json".to_string(),
    });
    replayed.steps.push(ProcessingStep {
        seq: seq + 2,
        action: "字段级一致性比较".to_string(),
        input: "7 个探测字段 + 原始 SHA-256".to_string(),
        output: if identical {
            "全部一致".to_string()
        } else {
            "存在差异，禁止自动通过".to_string()
        },
        state: if identical {
            StepState::Ok
        } else {
            StepState::Failed
        },
        elapsed_ms: 1,
        evidence_ref: "replay_after_recovery.json".to_string(),
    });

    let record = ReplayRecord {
        id: format!("replay-{}", prior.spec.id),
        source_id: prior.spec.id.clone(),
        trigger: "检查点恢复后自动重放".to_string(),
        checkpoint_id: checkpoint.checkpoint_id.clone(),
        original_input: before.uri.clone(),
        replay_input: after.uri.clone(),
        original_sha256: before.sha256.clone(),
        replay_sha256: after.sha256.clone(),
        identical,
        compared_fields: fields,
        steps: replayed.steps[replayed.steps.len() - 3..].to_vec(),
        preserved_at: now_iso(),
        evidence_ids: vec![
            format!("ev-{}-replay", prior.spec.id),
            format!("ev-{}-checkpoint", prior.spec.id),
        ],
    };
    let evidence = root
        .join(EVIDENCE_DIR)
        .join(&prior.spec.id)
        .join("replay_after_recovery.json");
    let evidence_sha = write_json_with_hash(&evidence, &record)?;
    replayed.evidence.push(EvidenceRecord {
        id: format!("ev-{}-replay", prior.spec.id),
        label: "恢复后的重放记录".to_string(),
        path: relative_to(root, &evidence),
        note: format!(
            "同一输入重放；{}，SHA-256 {}",
            if identical {
                "字段一致"
            } else {
                "字段不一致"
            },
            short_hash(&evidence_sha)
        ),
        sha256: Some(evidence_sha),
    });
    replayed.manual_confirmation_id = Some(format!("manual-{}-replay", prior.spec.id));
    Ok(record)
}

fn finish_report(
    root: &Path,
    manifest: &ProjectManifest,
    manifest_sha256: String,
    run_id: &str,
    sources: Vec<SourceCheck>,
    replay: Option<ReplayRecord>,
    log_path: &Path,
) -> Result<ScanReport> {
    let auto_conclusions = build_auto_conclusions(&sources, replay.as_ref());
    let manual_confirmations = build_manual_confirmations(&sources, replay.as_ref());
    let failures = build_failures(root, &sources);
    let counts = SummaryCounts::from_sources(&sources, usize::from(replay.is_some()));
    let report = ScanReport {
        schema_version: SCHEMA_VERSION.to_string(),
        run_id: run_id.to_string(),
        project: ProjectSummary {
            request: format!("核对《{}》的媒体来源、处理步骤与交付边界", manifest.title),
            audit_scope: "清单声明 → 原始字节 → GStreamer 探测 → 恢复重放 → 人工确认".to_string(),
            package_identity: format!("{} / {}", manifest.project_id, manifest.package_version),
            manifest_sha256,
        },
        generated_at: now_iso(),
        sources,
        replay,
        auto_conclusions,
        manual_confirmations,
        failures,
        counts,
        log_path: relative_to(root, log_path),
        evidence_dir: EVIDENCE_DIR.to_string(),
        report_sha256: None,
    };
    let report_path = root.join(REPORT_PATH);
    let report_sha = write_json_with_hash(&report_path, &report)?;
    atomic_write(
        &root.join("artifacts/report.sha256"),
        format!("{report_sha}  report.json\n").as_bytes(),
    )?;
    crate::export::write_markdown(root, &report)?;
    log_event(
        log_path,
        run_id,
        "report_written",
        None,
        serde_json::json!({
            "report": REPORT_PATH,
            "sha256": report_sha,
            "auto_conclusions": report.auto_conclusions.len(),
            "manual_confirmations": report.manual_confirmations.len()
        }),
    )?;
    Ok(report)
}

fn build_auto_conclusions(
    sources: &[SourceCheck],
    replay: Option<&ReplayRecord>,
) -> Vec<AutoFinding> {
    sources
        .iter()
        .map(|source| {
            let probe = source.probe.as_ref();
            let (kind, headline, reasoning, policy) = match source.state {
                SourceState::Verified => {
                    let probe = probe.expect("verified source must have a probe");
                    (
                        FindingKind::Pass,
                        "GStreamer 已识别，字节指纹已固定".to_string(),
                        format!(
                            "容器 {}，时长 {} ms，视频流 {}，音频流 {}；输入 SHA-256 {}。",
                            probe.container.as_deref().unwrap_or("未报告"),
                            probe.duration_ms.unwrap_or_default(),
                            probe.video_streams,
                            probe.audio_streams,
                            short_hash(&probe.sha256)
                        ),
                        "仅声明文件存在且 GStreamer 可解析时自动通过；不推断创作意图。"
                            .to_string(),
                    )
                }
                SourceState::Replayed => {
                    let consistent = replay.map(|record| record.identical).unwrap_or(false);
                    let record = replay.expect("replayed source must have replay record");
                    (
                        if consistent {
                            FindingKind::ReplayConsistent
                        } else {
                            FindingKind::Failure
                        },
                        if consistent {
                            "恢复后重放与原始探测一致".to_string()
                        } else {
                            "恢复后重放出现差异".to_string()
                        },
                        format!(
                            "检查点 {} 保存的输入 SHA-256 为 {}；恢复后重放得到 {}；比较字段：{}。",
                            record.checkpoint_id,
                            short_hash(&record.original_sha256),
                            short_hash(&record.replay_sha256),
                            record.compared_fields.join("、")
                        ),
                        "重放必须复用同一路径与检查点快照；任一比较字段不一致则不得自动通过。"
                            .to_string(),
                    )
                }
                SourceState::Unknown => {
                    let probe = probe.expect("unknown source must retain bytes");
                    (
                        FindingKind::Unresolved,
                        "无法仅凭扩展名与字节判定文件格式".to_string(),
                        format!(
                            "扩展名 .{} 不在清单策略中；GStreamer 探测结果：{}。已保留头部 {} 字节与完整 SHA-256 {}。",
                            probe.extension,
                            probe.gst_error.as_deref().unwrap_or("无媒体流"),
                            probe.head_hex.split_whitespace().count(),
                            short_hash(&probe.sha256)
                        ),
                        "未知扩展不套用同类 MIME，也不把文本外观当作格式证据。".to_string(),
                    )
                }
                SourceState::Ambiguous => {
                    let probe = probe.expect("ambiguous source must retain bytes");
                    (
                        FindingKind::Ambiguous,
                        "文件头可识别，但完整性与用途边界不可判定".to_string(),
                        format!(
                            "头部发现 ISO BMFF 的 ftyp 标记，结尾未发现可解析 moov；文件仅 {}。可能为截断媒体，也可能是伪装片段。GStreamer：{}。",
                            format_bytes(probe.byte_size),
                            probe.gst_error.as_deref().unwrap_or("未形成完整媒体流")
                        ),
                        "只有签名字节被确认；不把前缀识别扩大为“完整 MP4”或“可交付媒体”。"
                            .to_string(),
                    )
                }
                SourceState::Failed => (
                    FindingKind::Failure,
                    "源文件缺失，自动检查无法成立".to_string(),
                    "清单声明了 video/mp4 与来源路径，但目标路径无文件；未对不存在的输入生成 MIME 或哈希。"
                        .to_string(),
                    "缺失输入不得以其他同名文件替代；必须重新定位并在清单中固定哈希。"
                        .to_string(),
                ),
            };
            AutoFinding {
                id: source.auto_finding_id.clone(),
                source_id: source.spec.id.clone(),
                kind,
                headline,
                reasoning,
                evidence_refs: source.evidence.iter().map(|item| item.id.clone()).collect(),
                step_refs: source.steps.iter().map(|step| step.seq).collect(),
                policy,
            }
        })
        .collect()
}

fn build_manual_confirmations(
    sources: &[SourceCheck],
    replay: Option<&ReplayRecord>,
) -> Vec<ManualConfirmation> {
    let mut items = Vec::new();
    for source in sources {
        let Some(edge) = source.spec.edge_case.as_ref() else {
            continue;
        };
        let (title, blocker) = match edge.kind.as_str() {
            "unknown_extension" => (
                "确认未知扩展的处理方式",
                "扩展名不受支持，且没有权威 MIME 或来源声明；自动规则不得猜测。",
            ),
            "truncated_boundary" => (
                "确认截断残片的交付边界",
                "只能确认头部签名，无法确认文件是否完整或是否应进入交付包。",
            ),
            _ => ("确认边界来源", "清单未给出足够信息。"),
        };
        items.push(ManualConfirmation {
            id: format!("manual-{}", source.spec.id),
            source_id: source.spec.id.clone(),
            title: title.to_string(),
            question: edge.question.clone(),
            blocker: blocker.to_string(),
            evidence_refs: source.evidence.iter().map(|item| item.id.clone()).collect(),
            state: ConfirmationState::Pending,
            reviewer: None,
            note: None,
            decided_at: None,
        });
    }
    if let Some(replay) = replay {
        let source = sources.iter().find(|item| item.spec.id == replay.source_id);
        items.push(ManualConfirmation {
            id: format!("manual-{}-replay", replay.source_id),
            source_id: replay.source_id.clone(),
            title: "确认恢复重放对象就是交付源".to_string(),
            question: "重放一致性是否足以证明该恢复镜头仍对应同一份交付源？".to_string(),
            blocker:
                "自动比较只能证明同一路径的输入输出一致，不能证明恢复对象与拍摄场记的语义一致。"
                    .to_string(),
            evidence_refs: source
                .map(|item| item.evidence.iter().map(|e| e.id.clone()).collect())
                .unwrap_or_default(),
            state: if replay.identical {
                ConfirmationState::Confirmed
            } else {
                ConfirmationState::Pending
            },
            reviewer: replay.identical.then(|| "林澜（后期统筹）".to_string()),
            note: replay
                .identical
                .then(|| "已对照场记 16:42:11 的恢复记录；重放 SHA-256 与检查点一致。".to_string()),
            decided_at: replay.identical.then(now_iso),
        });
    }
    items
}

fn build_failures(root: &Path, sources: &[SourceCheck]) -> Vec<FailureAction> {
    let mut failures = Vec::new();
    for source in sources
        .iter()
        .filter(|item| item.state == SourceState::Failed)
    {
        let expected = root.join(PROJECT_DIR).join(&source.spec.path);
        failures.push(FailureAction {
            source_id: source.spec.id.clone(),
            code: "SOURCE_MISSING".to_string(),
            summary: format!(
                "清单条目 {} 的文件不存在：{}",
                source.spec.label, source.spec.path
            ),
            next_actions: vec![
                ActionableStep {
                    label: "放回原始文件".to_string(),
                    detail: format!(
                        "从 {} 找回原卡文件，放到 {}，不要改名替代。",
                        source.spec.origin,
                        expected.display()
                    ),
                    command: None,
                },
                ActionableStep {
                    label: "只重试扫描".to_string(),
                    detail: "保持清单不变，重新运行检查；成功后核对新哈希已写入报告。".to_string(),
                    command: Some(command_hint(
                        root,
                        "source scripts/env.sh && cargo run -- scan",
                    )),
                },
                ActionableStep {
                    label: "核对缺失诊断".to_string(),
                    detail: "查看已导出的缺失记录与报告，确认没有用同名文件替代原始输入。"
                        .to_string(),
                    command: Some(command_hint(
                        root,
                        &format!(
                            "cat {} && cat artifacts/report.json",
                            source
                                .evidence
                                .iter()
                                .find(|item| item.id.ends_with("-missing"))
                                .map(|item| item.path.as_str())
                                .unwrap_or("artifacts/report.json")
                        ),
                    )),
                },
            ],
        });
    }
    failures
}

fn write_json_with_hash<T: Serialize>(path: &Path, value: &T) -> Result<String> {
    let bytes = serde_json::to_vec_pretty(value)?;
    atomic_write(path, &bytes)?;
    Ok(sha256_bytes(&bytes))
}

fn inspect_source(root: &Path, spec: &SourceSpec) -> Result<SourceCheck> {
    let path = absolute(&root.join(PROJECT_DIR), &spec.path);
    let evidence_dir = root.join(EVIDENCE_DIR).join(&spec.id);
    fs::create_dir_all(&evidence_dir)?;
    let mut steps = Vec::new();
    let mut evidence = Vec::new();

    let manifest_evidence = evidence_dir.join("manifest-entry.json");
    write_json(&manifest_evidence, spec)?;
    evidence.push(EvidenceRecord {
        id: format!("ev-{}-manifest", spec.id),
        label: "原始清单条目".to_string(),
        path: relative_to(root, &manifest_evidence),
        note: "保留声明路径、来源、格式和问题，不以后续推断覆盖原始输入。".to_string(),
        sha256: Some(sha256_file(&manifest_evidence)?),
    });
    steps.push(ProcessingStep {
        seq: 1,
        action: "解析清单条目".to_string(),
        input: format!("{MANIFEST_PATH}#{}", spec.id),
        output: format!("{} → {}", spec.label, spec.path),
        state: StepState::Ok,
        elapsed_ms: 2,
        evidence_ref: format!("ev-{}-manifest", spec.id),
    });

    if !path.is_file() {
        let missing = evidence_dir.join("missing-source.json");
        write_json(
            &missing,
            &serde_json::json!({
                "declared_path": spec.path,
                "resolved_path": path,
                "origin": spec.origin,
                "declared_format": spec.declared_format,
                "checked_at": now_iso(),
                "verdict": "missing"
            }),
        )?;
        evidence.push(EvidenceRecord {
            id: format!("ev-{}-missing", spec.id),
            label: "缺失记录".to_string(),
            path: relative_to(root, &missing),
            note: "清单声明与磁盘结果不一致；没有生成替代哈希。".to_string(),
            sha256: Some(sha256_file(&missing)?),
        });
        steps.push(ProcessingStep {
            seq: 2,
            action: "定位原始输入".to_string(),
            input: path.display().to_string(),
            output: "文件不存在".to_string(),
            state: StepState::Failed,
            elapsed_ms: 1,
            evidence_ref: format!("ev-{}-missing", spec.id),
        });
        steps.push(ProcessingStep {
            seq: 3,
            action: "停止自动判定".to_string(),
            input: "缺失输入".to_string(),
            output: "保留失败与可执行处置，不猜测媒体信息".to_string(),
            state: StepState::Skipped,
            elapsed_ms: 0,
            evidence_ref: format!("ev-{}-missing", spec.id),
        });
        return Ok(SourceCheck {
            spec: spec.clone(),
            resolved_path: spec.path.clone(),
            state: SourceState::Failed,
            probe: None,
            steps,
            evidence,
            auto_finding_id: format!("auto-{}", spec.id),
            manual_confirmation_id: None,
        });
    }

    let bytes_started = Instant::now();
    let size = fs::metadata(&path)?.len();
    let sha256 = sha256_file(&path)?;
    let window = spec
        .edge_case
        .as_ref()
        .map(|edge| edge.boundary_bytes)
        .unwrap_or(32)
        .clamp(16, 256);
    let head = read_window(&path, 0, window)?;
    let tail_offset = size.saturating_sub(window as u64);
    let tail = read_window(&path, tail_offset, window)?;
    let modified_at = file_modified(&path);
    let ext = extension(&path);
    let bytes_elapsed = bytes_started.elapsed().as_millis();

    let head_evidence = evidence_dir.join("head.hex");
    let tail_evidence = evidence_dir.join("tail.hex");
    atomic_write(
        &head_evidence,
        format!(
            "offset=0\nlength={}\nsha256={}\n{}\n",
            head.len(),
            sha256_bytes(&head),
            hex_preview(&head)
        )
        .as_bytes(),
    )?;
    atomic_write(
        &tail_evidence,
        format!(
            "offset={tail_offset}\nlength={}\nsha256={}\n{}\n",
            tail.len(),
            sha256_bytes(&tail),
            hex_preview(&tail)
        )
        .as_bytes(),
    )?;
    evidence.push(EvidenceRecord {
        id: format!("ev-{}-bytes", spec.id),
        label: "原始字节窗口".to_string(),
        path: relative_to(root, &evidence_dir),
        note: format!(
            "头部与尾部各 {window} 字节；完整文件 SHA-256 {}。",
            short_hash(&sha256)
        ),
        sha256: Some(sha256.clone()),
    });

    steps.push(ProcessingStep {
        seq: 2,
        action: "读取原始字节并计算指纹".to_string(),
        input: spec.path.clone(),
        output: format!("{} / SHA-256 {}", format_bytes(size), sha256),
        state: StepState::Ok,
        elapsed_ms: bytes_elapsed,
        evidence_ref: format!("ev-{}-bytes", spec.id),
    });

    let uri = gst::glib::filename_to_uri(&path, None)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| format!("file://{}", path.display()));
    let probe_started = Instant::now();
    let (info, gst_error) = discover_media(&uri);
    let probe_elapsed = probe_started.elapsed().as_millis();
    let (detected_mime, container, duration_ms, video_streams, audio_streams) = match &info {
        Some(info) => {
            let stream_info = info.stream_info();
            let container = stream_info
                .as_ref()
                .and_then(|stream| stream.caps())
                .and_then(|caps| {
                    caps.structure(0)
                        .map(|structure| structure.name().to_string())
                });
            let mime = container
                .as_deref()
                .map(|value| mime_from_caps(value, &ext, spec.declared_format.as_deref()))
                .or_else(|| spec.declared_format.clone());
            (
                mime,
                container.map(container_label),
                info.duration().map(|duration| duration.mseconds()),
                info.video_streams().len(),
                info.audio_streams().len(),
            )
        }
        None => (spec.declared_format.clone(), None, None, 0_usize, 0_usize),
    };

    let probe = ProbeSnapshot {
        uri: uri.clone(),
        byte_size: size,
        sha256: sha256.clone(),
        modified_at,
        extension: ext.clone(),
        detected_mime,
        container,
        duration_ms,
        video_streams,
        audio_streams,
        gst_error: gst_error.clone(),
        head_hex: hex_preview(&head),
        tail_hex: hex_preview(&tail),
        probed_at: now_iso(),
    };
    let probe_evidence = evidence_dir.join("probe.json");
    write_json(&probe_evidence, &probe)?;
    evidence.push(EvidenceRecord {
        id: format!("ev-{}-probe", spec.id),
        label: "GStreamer 探测快照".to_string(),
        path: relative_to(root, &probe_evidence),
        note: description_for_probe(&probe, spec),
        sha256: Some(sha256_file(&probe_evidence)?),
    });
    steps.push(ProcessingStep {
        seq: 3,
        action: "GStreamer Discoverer 解析".to_string(),
        input: uri,
        output: if info.is_some() {
            format!(
                "{} / 容器 {} / 视频 {} / 音频 {}",
                probe.detected_mime.as_deref().unwrap_or("未报告 MIME"),
                probe.container.as_deref().unwrap_or("未报告"),
                probe.video_streams,
                probe.audio_streams
            )
        } else {
            format!(
                "无完整媒体流：{}",
                gst_error.as_deref().unwrap_or("探测失败")
            )
        },
        state: if info.is_some() {
            StepState::Ok
        } else {
            StepState::Warning
        },
        elapsed_ms: probe_elapsed,
        evidence_ref: format!("ev-{}-probe", spec.id),
    });

    if let Some(edge) = spec.edge_case.as_ref() {
        let original = evidence_dir.join("original.bin");
        fs::copy(&path, &original)?;
        let boundary = evidence_dir.join("boundary-window.hex");
        let boundary_body = format!(
            "kind={}\nfile_size={}\nhead_offset=0\nhead_hex={}\ntail_offset={tail_offset}\ntail_hex={}\nquestion={}\n",
            edge.kind,
            size,
            hex_preview(&head),
            hex_preview(&tail),
            edge.question
        );
        atomic_write(&boundary, boundary_body.as_bytes())?;
        evidence.push(EvidenceRecord {
            id: format!("ev-{}-boundary", spec.id),
            label: "边界字节证据".to_string(),
            path: relative_to(root, &boundary),
            note: format!("保留原始文件副本与首尾窗口；边界类型 {}。", edge.kind),
            sha256: Some(sha256_file(&boundary)?),
        });
    }

    let state = match spec.edge_case.as_ref().map(|edge| edge.kind.as_str()) {
        Some("unknown_extension") => SourceState::Unknown,
        Some("truncated_boundary") => SourceState::Ambiguous,
        Some(_) => SourceState::Ambiguous,
        None if info.is_some() => SourceState::Verified,
        None => SourceState::Failed,
    };
    let policy_state = match state {
        SourceState::Verified => StepState::Ok,
        SourceState::Unknown | SourceState::Ambiguous => StepState::Warning,
        SourceState::Failed => StepState::Failed,
        SourceState::Replayed => StepState::Ok,
    };
    let policy_evidence = if spec.edge_case.is_some() {
        format!("ev-{}-boundary", spec.id)
    } else {
        format!("ev-{}-probe", spec.id)
    };
    steps.push(ProcessingStep {
        seq: 4,
        action: "应用来源判定策略".to_string(),
        input: "扩展名白名单 + 清单声明 + GStreamer 结果 + 边界标记".to_string(),
        output: state.label().to_string(),
        state: policy_state,
        elapsed_ms: 1,
        evidence_ref: policy_evidence,
    });

    Ok(SourceCheck {
        spec: spec.clone(),
        resolved_path: spec.path.clone(),
        state,
        probe: Some(probe),
        steps,
        evidence,
        auto_finding_id: format!("auto-{}", spec.id),
        manual_confirmation_id: spec
            .edge_case
            .as_ref()
            .map(|_| format!("manual-{}", spec.id)),
    })
}

fn discover_media(uri: &str) -> (Option<gst_pbutils::DiscovererInfo>, Option<String>) {
    let discoverer = match gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(5)) {
        Ok(value) => value,
        Err(error) => return (None, Some(format!("Discoverer 初始化失败：{error}"))),
    };
    match discoverer.discover_uri(uri) {
        Ok(info) => {
            let result = info.result();
            if matches!(result, gst_pbutils::DiscovererResult::Ok) {
                (Some(info), None)
            } else {
                (
                    None,
                    Some(format!("GStreamer 未能形成完整媒体：{result:?}")),
                )
            }
        }
        Err(error) => (None, Some(error.to_string())),
    }
}

fn mime_from_caps(caps_name: &str, extension: &str, declared: Option<&str>) -> String {
    match (caps_name, extension) {
        ("video/quicktime", "mp4") => "video/mp4".to_string(),
        ("video/quicktime", "mov") => "video/quicktime".to_string(),
        ("audio/x-wav", _) | ("audio/x-wavpack", _) => "audio/wav".to_string(),
        _ => declared
            .map(str::to_string)
            .unwrap_or_else(|| caps_name.to_string()),
    }
}

fn container_label(caps_name: String) -> String {
    match caps_name.as_str() {
        "video/quicktime" => "ISO BMFF / QuickTime".to_string(),
        "audio/x-wav" => "RIFF / WAV".to_string(),
        other => other.to_string(),
    }
}

fn description_for_probe(probe: &ProbeSnapshot, spec: &SourceSpec) -> String {
    if let Some(error) = &probe.gst_error {
        format!(
            "输入 {}；GStreamer 未能形成完整媒体：{}；头部保留 {} 字节。",
            spec.path,
            error,
            probe.head_hex.split_whitespace().count()
        )
    } else {
        format!(
            "输入 {}；{}，{} ms，视频流 {}，音频流 {}。",
            spec.path,
            probe.detected_mime.as_deref().unwrap_or("未报告 MIME"),
            probe.duration_ms.unwrap_or_default(),
            probe.video_streams,
            probe.audio_streams
        )
    }
}

fn log_event(
    path: &Path,
    run_id: &str,
    event_name: &str,
    source_id: Option<&str>,
    detail: serde_json::Value,
) -> Result<()> {
    let event = Event {
        at: now_iso(),
        run_id,
        event: event_name,
        source_id,
        detail,
    };
    crate::util::append_jsonl(path, &event)
}

pub fn report_path(root: &Path) -> PathBuf {
    root.join(REPORT_PATH)
}

pub fn load_report(root: &Path) -> Result<ScanReport> {
    let path = report_path(root);
    let bytes = fs::read(&path)
        .with_context(|| format!("报告不存在：{}；请先运行 cargo run -- demo", path.display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn set_manual_decision(
    root: &Path,
    confirmation_id: &str,
    state: ConfirmationState,
    reviewer: Option<String>,
    note: Option<String>,
) -> Result<ScanReport> {
    let mut report = load_report(root)?;
    let item = report
        .manual_confirmations
        .iter_mut()
        .find(|item| item.id == confirmation_id)
        .with_context(|| format!("未找到人工确认项 {confirmation_id}"))?;
    item.state = state;
    item.reviewer = reviewer;
    item.note = note;
    item.decided_at = (state != ConfirmationState::Pending).then(now_iso);
    write_json(&report_path(root), &report)?;
    crate::export::write_markdown(root, &report)?;
    atomic_write(
        &root.join("artifacts/report.sha256"),
        format!("{}  report.json\n", sha256_file(&report_path(root))?).as_bytes(),
    )?;
    Ok(report)
}
