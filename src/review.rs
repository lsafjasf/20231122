use crate::model::{Record, SESSION_V2, Session, SourceRef, Track, canonical_json, sha256_hex};
use crate::replay::{FirstError, Phase, ReplayReport};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

pub const SUMMARY_SCHEMA: &str = "frame-loss-review/summary-v2";
pub const MANIFEST_SCHEMA: &str = "frame-loss-review/manifest-v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileDigest {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Manifest {
    pub schema: String,
    pub algorithm: String,
    pub files: BTreeMap<String, FileDigest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FirstDivergence {
    pub index: usize,
    pub reason: String,
    pub control_record: Option<String>,
    pub observed_record: Option<String>,
    pub control_excerpt: String,
    pub observed_excerpt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScenarioSummary {
    pub name: String,
    pub description: String,
    pub accepted_steps: usize,
    pub input_steps: usize,
    pub waveform_sha256: String,
    pub first_error: Option<FirstError>,
    pub first_divergence: Option<FirstDivergence>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecoveredSummary {
    pub accepted_steps: usize,
    pub observed_frames: u64,
    pub compensated_frames: u64,
    pub final_phase: Phase,
    pub waveform_sha256: String,
    pub first_error: Option<FirstError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TruncationStats {
    pub total_bytes: usize,
    pub complete_prefix_records: usize,
    pub incomplete_tail_bytes: usize,
    pub final_newline: bool,
    pub raw_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TruncationSummary {
    pub stats: TruncationStats,
    pub complete_record_ids: Vec<String>,
    pub first_error: Option<FirstError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompatibilitySummary {
    pub legacy_source_schema: String,
    pub migrated: bool,
    pub warnings: Vec<String>,
    pub new_schema: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewSummary {
    pub schema: String,
    pub has_total_score: bool,
    pub no_score_reason: String,
    pub recovered: RecoveredSummary,
    pub truncation: TruncationSummary,
    pub compatibility: CompatibilitySummary,
    pub first_error_across_scenarios: Option<FirstError>,
    pub scenarios: Vec<ScenarioSummary>,
}

pub struct ExperimentInputs<'a> {
    pub recovered_session: &'a Session,
    pub recovered_bytes: &'a [u8],
    pub truncated_bytes: &'a [u8],
    pub truncated_read: &'a crate::input::JsonlRead,
    pub legacy_bytes: &'a [u8],
    pub legacy_session: &'a Session,
    pub recovered_report: &'a ReplayReport,
    pub truncated_report: &'a ReplayReport,
    pub legacy_report: &'a ReplayReport,
    pub tampered_report: &'a ReplayReport,
}

pub fn session_from_records(
    session_id: &str,
    source_path: &str,
    bytes: &[u8],
    records: &[Record],
    notes: Vec<String>,
) -> Result<Session> {
    let first = records.first().context("session has no records")?;
    let track = records
        .iter()
        .find(|record| record.track.id == first.track.id)
        .map(|record| record.track.clone())
        .unwrap_or_else(|| Track {
            id: first.track.id.clone(),
            index: first.track.index,
            kind: first.track.kind.clone(),
            caps: first.track.caps.clone(),
        });
    Ok(Session {
        schema: SESSION_V2.to_string(),
        session_id: session_id.to_string(),
        source: SourceRef {
            path: source_path.to_string(),
            size_bytes: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        },
        track,
        params: first.params.clone(),
        records: records.to_vec(),
        compatibility: None,
        notes,
    })
}

pub fn records_jsonl(records: &[Record]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    for record in records {
        serde_json::to_writer(&mut output, record)?;
        output.push(b'\n');
    }
    Ok(output)
}

pub fn write_trace(path: &Path, report: &ReplayReport) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("create trace {}", path.display()))?;
    for step in &report.steps {
        serde_json::to_writer(&mut file, step)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

pub fn compare_reports(control: &ReplayReport, observed: &ReplayReport) -> Option<FirstDivergence> {
    let count = control.steps.len().max(observed.steps.len());
    for index in 0..count {
        let control_step = control.steps.get(index);
        let observed_step = observed.steps.get(index);
        let (Some(control_step), Some(observed_step)) = (control_step, observed_step) else {
            return Some(FirstDivergence {
                index,
                reason: "input_missing".to_string(),
                control_record: control_step.map(record_descriptor),
                observed_record: observed_step.map(record_descriptor),
                control_excerpt: control_step
                    .map(outcome_excerpt)
                    .unwrap_or_else(|| "EOF".to_string()),
                observed_excerpt: "EOF".to_string(),
            });
        };
        if observed_step.outcome.status != control_step.outcome.status {
            return Some(FirstDivergence {
                index,
                reason: "outcome_mismatch".to_string(),
                control_record: Some(record_descriptor(control_step)),
                observed_record: Some(record_descriptor(observed_step)),
                control_excerpt: outcome_excerpt(control_step),
                observed_excerpt: outcome_excerpt(observed_step),
            });
        }
        if semantic_input(&control_step.input) != semantic_input(&observed_step.input) {
            return Some(FirstDivergence {
                index,
                reason: "record_mismatch".to_string(),
                control_record: Some(record_descriptor(control_step)),
                observed_record: Some(record_descriptor(observed_step)),
                control_excerpt: outcome_excerpt(control_step),
                observed_excerpt: outcome_excerpt(observed_step),
            });
        }
    }
    None
}

fn semantic_input(input: &Value) -> Value {
    let frame = input.get("frame").cloned().unwrap_or(Value::Null);
    let track = input.get("track").cloned().unwrap_or(Value::Null);
    let compensation = input
        .get("compensation")
        .cloned()
        .map(|value| {
            json!({
                "applied": value.get("applied"),
                "mode": value.get("mode"),
                "gap_position": value.get("gap_position"),
                "gap_length": value.get("gap_length"),
                "blend": value.get("blend"),
                "reference_before": value.get("reference_before"),
                "reference_after": value.get("reference_after"),
                "output_luma": value.get("output_luma"),
            })
        })
        .unwrap_or(Value::Null);
    json!({
        "sequence": input.get("sequence"),
        "track": {
            "id": track.get("id"),
            "index": track.get("index"),
            "kind": track.get("kind"),
        },
        "frame": frame,
        "params_fingerprint": input.get("params_fingerprint"),
        "compensation": compensation,
    })
}

fn record_descriptor(step: &crate::replay::ReplayStep) -> String {
    let input = &step.input;
    match (
        input.get("sequence").and_then(Value::as_u64),
        input.get("track"),
        input.get("frame"),
    ) {
        (Some(sequence), Some(track), Some(frame)) => format!(
            "seq={sequence} frame={} track={} time_ms={}",
            frame.get("frame").and_then(Value::as_u64).unwrap_or(0),
            track.get("id").and_then(Value::as_str).unwrap_or("?"),
            frame.get("time_ms").and_then(Value::as_u64).unwrap_or(0)
        ),
        _ => format!(
            "input_kind={}",
            input
                .get("input_kind")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
        ),
    }
}

fn outcome_excerpt(step: &crate::replay::ReplayStep) -> String {
    if let Some(effect) = &step.outcome.effect {
        effect.clone()
    } else {
        format!(
            "{}: {}",
            step.outcome.rule.as_deref().unwrap_or("unknown"),
            step.outcome.message.as_deref().unwrap_or("")
        )
    }
}

pub fn build_summary(inputs: &ExperimentInputs<'_>) -> ReviewSummary {
    let mut scenarios = vec![
        scenario_summary(
            inputs.truncated_report,
            compare_reports(inputs.recovered_report, inputs.truncated_report),
        ),
        scenario_summary(
            inputs.legacy_report,
            compare_reports(inputs.recovered_report, inputs.legacy_report),
        ),
        scenario_summary(
            inputs.tampered_report,
            compare_reports(inputs.recovered_report, inputs.tampered_report),
        ),
    ];
    scenarios.sort_by(|left, right| left.name.cmp(&right.name));
    let first_error_across_scenarios = scenarios
        .iter()
        .filter_map(|scenario| scenario.first_error.clone())
        .min_by_key(|error| error.step_index);
    let complete_prefix_records = inputs.truncated_read.records.len();
    let incomplete_tail_bytes = inputs
        .truncated_read
        .error
        .as_ref()
        .map_or(0, |error| error.raw.len());
    let compatibility = inputs
        .legacy_session
        .compatibility
        .as_ref()
        .map(|compat| CompatibilitySummary {
            legacy_source_schema: compat.source_schema.clone(),
            migrated: compat.migrated,
            warnings: compat.warnings.clone(),
            new_schema: SESSION_V2.to_string(),
        })
        .unwrap_or(CompatibilitySummary {
            legacy_source_schema: "frame-loss-review/v1".to_string(),
            migrated: false,
            warnings: Vec::new(),
            new_schema: SESSION_V2.to_string(),
        });
    ReviewSummary {
        schema: SUMMARY_SCHEMA.to_string(),
        has_total_score: false,
        no_score_reason:
            "A total score would hide the first invalid state; this report preserves each step and its evidence chain."
                .to_string(),
        recovered: RecoveredSummary {
            accepted_steps: inputs.recovered_report.accepted_steps,
            observed_frames: inputs
                .recovered_report
                .steps
                .last()
                .map_or(0, |step| step.state_after.frames_observed),
            compensated_frames: inputs
                .recovered_report
                .steps
                .last()
                .map_or(0, |step| step.state_after.compensated_frames),
            final_phase: inputs.recovered_report.final_phase,
            waveform_sha256: crate::replay::waveform_digest(&inputs.recovered_report.waveform),
            first_error: inputs.recovered_report.first_error.clone(),
        },
        truncation: TruncationSummary {
            stats: TruncationStats {
                total_bytes: inputs.truncated_read.total_bytes,
                complete_prefix_records,
                incomplete_tail_bytes,
                final_newline: inputs.truncated_read.final_newline,
                raw_sha256: inputs.truncated_read.raw_sha256.clone(),
            },
            complete_record_ids: inputs
                .truncated_read
                .records
                .iter()
                .filter_map(|record| {
                    record
                        .value
                        .get("record_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect(),
            first_error: inputs.truncated_report.first_error.clone(),
        },
        compatibility,
        first_error_across_scenarios,
        scenarios,
    }
}

fn scenario_summary(report: &ReplayReport, divergence: Option<FirstDivergence>) -> ScenarioSummary {
    ScenarioSummary {
        name: report.scenario.clone(),
        description: report.description.clone(),
        accepted_steps: report.accepted_steps,
        input_steps: report.input_steps,
        waveform_sha256: crate::replay::waveform_digest(&report.waveform),
        first_error: report.first_error.clone(),
        first_divergence: divergence,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyEvidence {
    pub algorithm: String,
    pub truncated_record_sha256: String,
    pub recovered_record_sha256: String,
    pub legacy_v1_sha256: String,
    pub recovered_waveform_sha256: String,
    pub legacy_import_waveform_sha256: String,
    pub recovered_evidence_chain_tip: String,
    pub truncated_first_error: Option<FirstError>,
}

pub fn key_evidence(inputs: &ExperimentInputs<'_>) -> KeyEvidence {
    KeyEvidence {
        algorithm: "sha256".to_string(),
        truncated_record_sha256: sha256_hex(inputs.truncated_bytes),
        recovered_record_sha256: sha256_hex(inputs.recovered_bytes),
        legacy_v1_sha256: sha256_hex(inputs.legacy_bytes),
        recovered_waveform_sha256: crate::replay::waveform_digest(
            &inputs.recovered_report.waveform,
        ),
        legacy_import_waveform_sha256: crate::replay::waveform_digest(
            &inputs.legacy_report.waveform,
        ),
        recovered_evidence_chain_tip: inputs.recovered_report.evidence_tip(),
        truncated_first_error: inputs.truncated_report.first_error.clone(),
    }
}

pub fn write_experiment(out_dir: &Path, inputs: &ExperimentInputs<'_>) -> Result<()> {
    fs::create_dir_all(out_dir)
        .with_context(|| format!("create output directory {}", out_dir.display()))?;
    write_bytes(
        &out_dir.join("recovered-record.jsonl"),
        inputs.recovered_bytes,
    )?;
    write_bytes(
        &out_dir.join("truncated-record.jsonl"),
        inputs.truncated_bytes,
    )?;
    write_bytes(&out_dir.join("legacy-v1.json"), inputs.legacy_bytes)?;
    write_pretty(
        &out_dir.join("recovered-session-v2.json"),
        inputs.recovered_session,
    )?;
    write_pretty(
        &out_dir.join("legacy-import-v2.json"),
        inputs.legacy_session,
    )?;
    write_trace(
        &out_dir.join("recovered.trace.jsonl"),
        inputs.recovered_report,
    )?;
    write_trace(
        &out_dir.join("truncated-prefix.trace.jsonl"),
        inputs.truncated_report,
    )?;
    write_trace(
        &out_dir.join("legacy-v1-import.trace.jsonl"),
        inputs.legacy_report,
    )?;
    write_trace(
        &out_dir.join("tampered-params.trace.jsonl"),
        inputs.tampered_report,
    )?;
    crate::waveform::write_waveform_csv(
        &out_dir.join("waveform-recovered.csv"),
        &inputs.recovered_report.waveform,
    )?;
    crate::waveform::write_waveform_svg(
        &out_dir.join("waveform-recovered.svg"),
        inputs.recovered_report,
        "Recovered replay / GStreamer output luma waveform",
        false,
    )?;
    crate::waveform::write_waveform_csv(
        &out_dir.join("waveform-legacy.csv"),
        &inputs.legacy_report.waveform,
    )?;
    crate::waveform::write_waveform_svg(
        &out_dir.join("waveform-legacy.svg"),
        inputs.legacy_report,
        "Legacy v1 import / reconstructed output luma",
        true,
    )?;

    let summary = build_summary(inputs);
    write_pretty(&out_dir.join("summary.json"), &summary)?;
    let evidence = key_evidence(inputs);
    write_pretty(&out_dir.join("key-evidence.json"), &evidence)?;
    write_text(&out_dir.join("report.md"), &render_report(&summary, inputs))?;
    let manifest = create_manifest(out_dir, &["manifest.sha256.json"])?;
    write_pretty(&out_dir.join("manifest.sha256.json"), &manifest)?;
    Ok(())
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).with_context(|| format!("write {}", path.display()))
}

fn write_text(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

fn write_pretty<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut output = serde_json::to_vec_pretty(value)?;
    output.push(b'\n');
    write_bytes(path, &output)
}

pub fn render_report(summary: &ReviewSummary, inputs: &ExperimentInputs<'_>) -> String {
    let mut report = String::new();
    report.push_str("# 视频帧丢失补偿复核报告\n\n");
    report.push_str(
        "本报告不生成总分。它保留每次回放的输入、进入前状态、拒绝后状态、证据哈希，并以恢复后的完整重放作为对照。技术链路固定为 Rust + GStreamer + GTK4。\n\n",
    );
    report.push_str("## 恢复后的重放\n\n");
    report.push_str(&format!(
        "- 场景：`recovered-replay`；接受输入：{} / {}；最终阶段：`{:?}`；首错：{}\n",
        summary.recovered.accepted_steps,
        summary.recovered.accepted_steps,
        summary.recovered.final_phase,
        first_error_label(summary.recovered.first_error.as_ref())
    ));
    report.push_str(&format!(
        "- 观测帧：{}，已补偿帧：{}\n",
        summary.recovered.observed_frames, summary.recovered.compensated_frames
    ));
    report.push_str(&format!(
        "- 输出波形 SHA-256：`{}`\n",
        summary.recovered.waveform_sha256
    ));
    report.push_str(&format!(
        "- 证据链末端：`{}`\n\n",
        inputs.recovered_report.evidence_tip()
    ));
    report.push_str("### 时间点 / 轨道 / 帧 / 参数 / 波形对应\n\n");
    report.push_str("| sample | time_ms | pts_ns | track | frame | state | params | mode | output_luma | measured | record | evidence |\n");
    report.push_str("|---:|---:|---:|---|---:|---|---|---|---:|---:|---|---|\n");
    for point in &inputs.recovered_report.waveform {
        report.push_str(&format!(
            "| {} | {} | {} | {}:{} | {} | {} | `{}` | {} | {:.6} | {:.6} | `{}` | `{}` |\n",
            point.sample,
            point.time_ms,
            point.pts_ns,
            point.track_id,
            point.track_index,
            point.frame,
            frame_state_name(point.source_state),
            point.params_fingerprint,
            point.compensation_mode,
            point.output_luma,
            point.measured_luma,
            point.record_id,
            short(&point.evidence_sha256)
        ));
    }

    report.push_str("\n## 被截断的记录\n\n");
    report.push_str(&format!(
        "- 原始字节：{}；完整前缀记录：{}；截断尾部：{} bytes；文件以换行结束：{}\n",
        summary.truncation.stats.total_bytes,
        summary.truncation.stats.complete_prefix_records,
        summary.truncation.stats.incomplete_tail_bytes,
        summary.truncation.stats.final_newline
    ));
    report.push_str(&format!(
        "- 截断文件 SHA-256：`{}`\n",
        summary.truncation.stats.raw_sha256
    ));
    report.push_str(&format!(
        "- 完整前缀记录：`{}`\n",
        summary.truncation.complete_record_ids.join("`, `")
    ));
    if let Some(error) = &summary.truncation.first_error {
        report.push_str("\n**首个错误状态**\n\n");
        report.push_str(&render_first_error(error));
    }

    report.push_str("\n## 旧格式与新格式\n\n");
    report.push_str(&format!(
        "- 旧格式：`{}`；新格式：`{}`。\n",
        summary.compatibility.legacy_source_schema, summary.compatibility.new_schema
    ));
    report.push_str("- 旧字段 `before/after/mix/confidence/max_gap` 映射到 `before_frames/after_frames/blend/min_confidence/max_conceal_frames`；原始文件与 SHA-256 同时保留。\n");
    report.push_str(&format!(
        "- v1 迁移状态：{}；兼容警告 {} 条。\n",
        if summary.compatibility.migrated {
            "已迁移"
        } else {
            "未迁移"
        },
        summary.compatibility.warnings.len()
    ));
    for warning in &summary.compatibility.warnings {
        report.push_str(&format!("- 兼容警告：{}\n", escape_markdown(warning)));
    }

    report.push_str("\n## 首个错误而不是总分\n\n");
    report.push_str(&format!(
        "- `has_total_score = false`。{}\n",
        summary.no_score_reason
    ));
    if let Some(error) = &summary.first_error_across_scenarios {
        report.push_str(&format!(
            "- 跨场景最早首错：`{}` / 步骤 {} / `{}`。\n",
            error.scenario, error.step_index, error.rule
        ));
        report.push_str(&format!(
            "- 最后有效状态：`{}`；首个错误状态：`{}`。\n",
            state_line(&error.state_before),
            state_line(&error.state_after)
        ));
    } else {
        report.push_str("- 跨场景首错：none。\n");
    }

    report.push_str("\n| 场景 | 输入数 | 接受数 | 波形 SHA-256 | 首错 | 首个分歧 |\n");
    report.push_str("|---|---:|---:|---|---|---|\n");
    for scenario in &summary.scenarios {
        report.push_str(&format!(
            "| {} | {} | {} | `{}` | {} | {} |\n",
            scenario.name,
            scenario.input_steps,
            scenario.accepted_steps,
            short(&scenario.waveform_sha256),
            scenario
                .first_error
                .as_ref()
                .map_or("none".to_string(), |error| format!(
                    "步骤 {} `{}`",
                    error.step_index, error.rule
                )),
            scenario
                .first_divergence
                .as_ref()
                .map_or("none".to_string(), |divergence| {
                    format!(
                        "#{} {} / control={} / observed={}",
                        divergence.index,
                        divergence.reason,
                        divergence.control_record.as_deref().unwrap_or("EOF"),
                        divergence.observed_record.as_deref().unwrap_or("EOF")
                    )
                })
        ));
    }

    report.push_str("\n## 关键证据\n\n");
    report.push_str("- `truncated-record.jsonl`：未补写的物理截断输入。\n");
    report.push_str("- `recovered-record.jsonl`：恢复后的完整输入与控制重放。\n");
    report.push_str("- `legacy-v1.json` / `legacy-import-v2.json`：旧格式原文与迁移结果。\n");
    report.push_str("- `*.trace.jsonl`：每步输入、进入前状态、结果、拒绝后状态和链式证据哈希。\n");
    report.push_str("- `waveform-recovered.csv` / `.svg`：逐帧输出波形及时间、轨道、帧、参数对应；`measured_luma` 来自 GStreamer GRAY8 输出。\n");
    report.push_str("- `summary.json`、`key-evidence.json`、`manifest.sha256.json`：结构化结论、关键证据指纹与全包校验。\n\n");
    report.push_str("复核命令：`cargo run -- verify-manifest artifacts/experiment/manifest.sha256.json`；GTK4 界面：`cargo run --features desktop --bin frame-loss-review -- desktop artifacts/experiment`。\n");
    report
}

fn frame_state_name(state: crate::model::FrameState) -> &'static str {
    match state {
        crate::model::FrameState::Present => "present",
        crate::model::FrameState::Missing => "missing",
    }
}

fn render_first_error(error: &FirstError) -> String {
    format!(
        "- 场景：`{}`\n- 步骤：`{}`\n- 记录：`{}`\n- 规则：`{}`\n- 期望：{}\n- 实际：{}\n- 进入前状态：`{}`\n- 拒绝后状态：`{}`\n- 本步证据：`{}`\n",
        error.scenario,
        error.step_index,
        error.record_id.as_deref().unwrap_or("（截断行）"),
        error.rule,
        escape_markdown(&error.expected),
        escape_markdown(&error.observed),
        state_line(&error.state_before),
        state_line(&error.state_after),
        error.evidence_sha256
    )
}

fn state_line(state: &crate::replay::ReplayState) -> String {
    format!(
        "phase={:?} cursor={} accepted={} last_record={} missing={} compensated={} first_error={}",
        state.phase,
        state.cursor,
        state.accepted,
        state.last_record_id.as_deref().unwrap_or("none"),
        state.missing_frames,
        state.compensated_frames,
        state
            .first_error
            .as_ref()
            .map_or("none".to_string(), |error| error.rule.clone())
    )
}

fn first_error_label(error: Option<&FirstError>) -> String {
    error.map_or("none".to_string(), |error| {
        format!("步骤 {} `{}`", error.step_index, error.rule)
    })
}

fn short(hash: &str) -> &str {
    hash.get(..12).unwrap_or(hash)
}

fn escape_markdown(input: &str) -> String {
    input.replace('|', "\\|").replace('\n', " ")
}

pub fn create_manifest(dir: &Path, excluded: &[&str]) -> Result<Manifest> {
    let mut files = BTreeMap::new();
    for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if excluded.contains(&name.as_str()) {
            continue;
        }
        let bytes = fs::read(entry.path())?;
        files.insert(
            name,
            FileDigest {
                bytes: bytes.len() as u64,
                sha256: sha256_hex(&bytes),
            },
        );
    }
    Ok(Manifest {
        schema: MANIFEST_SCHEMA.to_string(),
        algorithm: "sha256".to_string(),
        files,
    })
}

pub fn verify_manifest(path: &Path) -> Result<Vec<String>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)?;
    if manifest.algorithm != "sha256" || manifest.schema != MANIFEST_SCHEMA {
        bail!("unsupported manifest schema or algorithm");
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut failures = Vec::new();
    for (name, expected) in &manifest.files {
        let file_path = dir.join(name);
        match fs::read(&file_path) {
            Ok(actual) => {
                let actual_hash = sha256_hex(&actual);
                if actual.len() as u64 != expected.bytes || actual_hash != expected.sha256 {
                    failures.push(format!(
                        "{}: expected {} bytes sha256={}, observed {} bytes sha256={}",
                        name,
                        expected.bytes,
                        expected.sha256,
                        actual.len(),
                        actual_hash
                    ));
                }
            }
            Err(error) => failures.push(format!("{}: cannot read: {error}", name)),
        }
    }
    for name in manifest
        .files
        .keys()
        .filter(|name| name.ends_with(".trace.jsonl"))
    {
        match verify_trace(&dir.join(name)) {
            Ok(trace_failures) => failures.extend(
                trace_failures
                    .into_iter()
                    .map(|failure| format!("{}: {failure}", name)),
            ),
            Err(error) => failures.push(format!("{}: cannot verify trace: {error}", name)),
        }
    }
    Ok(failures)
}

pub fn verify_trace(path: &Path) -> Result<Vec<String>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("read trace {}", path.display()))?;
    let mut failures = Vec::new();
    let mut steps: Vec<crate::replay::ReplayStep> = Vec::new();
    for (line_index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let step: crate::replay::ReplayStep = serde_json::from_str(line)
            .with_context(|| format!("parse {} line {}", path.display(), line_index + 1))?;
        if step.index != steps.len() as u64 {
            failures.push(format!(
                "line {}: step index {} does not match position {}",
                line_index + 1,
                step.index,
                steps.len()
            ));
        }
        if step.expected_sequence != Some(step.state_before.cursor) {
            failures.push(format!(
                "step {}: expected_sequence {:?} does not match state_before.cursor {}",
                step.index, step.expected_sequence, step.state_before.cursor
            ));
        }
        if let Some(previous) = steps.last() {
            if step.evidence.previous_evidence_sha256 != previous.evidence.evidence_sha256 {
                failures.push(format!(
                    "step {}: previous evidence hash does not match prior step",
                    step.index
                ));
            }
            if step.state_before != previous.state_after {
                failures.push(format!(
                    "step {}: state_before does not match prior state_after",
                    step.index
                ));
            }
        } else if step.evidence.previous_evidence_sha256 != "0".repeat(64) {
            failures.push("step 0: initial previous evidence hash is not zero".to_string());
        }
        let recomputed = crate::replay::build_evidence(
            &step.evidence.previous_evidence_sha256,
            &canonical_json(&step.input),
            &canonical_json(&step.state_before),
            &canonical_json(&step.state_after),
            &canonical_json(&step.outcome),
        );
        if recomputed != step.evidence {
            failures.push(format!(
                "step {}: evidence hashes do not reproduce from input/state/result",
                step.index
            ));
        }
        if step.outcome.status == "rejected" {
            let Some(rule) = &step.outcome.rule else {
                failures.push(format!("step {}: rejected without a rule", step.index));
                steps.push(step);
                continue;
            };
            if step.state_after.phase != Phase::Faulted {
                failures.push(format!(
                    "step {}: rejected outcome did not freeze phase=Faulted",
                    step.index
                ));
            }
            if step
                .state_after
                .first_error
                .as_ref()
                .map(|error| &error.rule)
                != Some(rule)
            {
                failures.push(format!(
                    "step {}: rejected rule is not stored as first_error",
                    step.index
                ));
            }
            if step.state_before.first_error.is_some() {
                failures.push(format!(
                    "step {}: state_before already contains a prior error",
                    step.index
                ));
            }
        }
        steps.push(step);
    }
    Ok(failures)
}
