use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::SCHEMA;
use crate::evidence::sha256_text;
use crate::fixture::{demo_record, demo_wave_bytes, load_truncated_bytes, truncated_fixture_bytes};
use crate::model::{
    Action, FirstError, Phase, RecordedInput, RecoveredRecord, ReplayRun, ReplayState, StepTrace,
};
use crate::replay::{replay_inputs, replay_record};

pub const TRUNCATED_COMPLETE_INPUTS: usize = 5;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceDivergence {
    pub index: usize,
    pub control_step_id: String,
    pub control_action: String,
    pub observed_step_id: Option<String>,
    pub observed_action: Option<String>,
    pub reason: String,
    pub control_outcome: String,
    pub observed_outcome: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefixComparison {
    pub compared_steps: usize,
    pub matched_steps: usize,
    pub first_divergence: Option<TraceDivergence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MutationKind {
    TailCut,
    SwapInputs {
        left: usize,
        right: usize,
    },
    DuplicateSubmission {
        source_index: usize,
        inserted_at: usize,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scenario {
    pub name: String,
    pub description: String,
    pub mutation: MutationKind,
    pub run: ReplayRun,
    pub comparison: PrefixComparison,
}

#[derive(Clone, Debug)]
pub struct Experiment {
    pub record: RecoveredRecord,
    pub truncated_bytes: Vec<u8>,
    pub truncated: crate::fixture::LoadedTruncatedRecord,
    pub control: ReplayRun,
    pub scenarios: Vec<Scenario>,
}

impl Experiment {
    pub fn builtin() -> Result<Self, io::Error> {
        let record = demo_record();
        Self::from_record(record)
    }

    pub fn from_record(record: RecoveredRecord) -> Result<Self, io::Error> {
        validate_recovered_record(&record)?;
        let truncated_bytes = truncated_fixture_bytes(&record, TRUNCATED_COMPLETE_INPUTS)?;
        let truncated = load_truncated_bytes(&truncated_bytes)?;
        let control = replay_record(&record, "recovered-control");
        let mut scenarios = Vec::new();

        let truncated_run = replay_inputs(
            &record.record_id,
            "truncated-prefix",
            &truncated.replay_inputs(),
        );
        scenarios.push(Scenario {
            name: "truncated-prefix".to_owned(),
            description: "只回放截断文件中完整落盘的前缀，不补写缺失尾部。".to_owned(),
            mutation: MutationKind::TailCut,
            comparison: compare_runs(&truncated_run, &control),
            run: truncated_run,
        });

        let (reordered_inputs, left, right) = swap_inputs(&record.inputs, 2, 3)?;
        let reordered_run = replay_inputs(&record.record_id, "reordered", &reordered_inputs);
        scenarios.push(Scenario {
            name: "reordered".to_owned(),
            description: "交换 title 与 artist 两条元数据提交，保留原始 sequence。".to_owned(),
            mutation: MutationKind::SwapInputs { left, right },
            comparison: compare_runs(&reordered_run, &control),
            run: reordered_run,
        });

        let (duplicate_inputs, source_index, inserted_at) = duplicate_submission(&record.inputs)?;
        let duplicate_run =
            replay_inputs(&record.record_id, "duplicate-submission", &duplicate_inputs);
        scenarios.push(Scenario {
            name: "duplicate-submission".to_owned(),
            description: "在 commit 后再次提交同一 submission_id，并推进 sequence 以排除乱序干扰。"
                .to_owned(),
            mutation: MutationKind::DuplicateSubmission {
                source_index,
                inserted_at,
            },
            comparison: compare_runs(&duplicate_run, &control),
            run: duplicate_run,
        });

        Ok(Self {
            record,
            truncated_bytes,
            truncated,
            control,
            scenarios,
        })
    }

    pub fn scenario(&self, name: &str) -> Option<&Scenario> {
        self.scenarios.iter().find(|scenario| scenario.name == name)
    }

    pub fn first_error_across_scenarios(&self) -> Option<FirstErrorAt> {
        self.scenarios
            .iter()
            .filter_map(|scenario| {
                scenario
                    .run
                    .first_error
                    .as_ref()
                    .map(|error| (scenario, error))
            })
            .min_by_key(|(_, error)| error.step_index)
            .map(|(scenario, error)| FirstErrorAt {
                scenario: scenario.name.clone(),
                step_index: error.step_index,
                step_id: error.step_id.clone(),
                rule: error.rule.clone(),
                message: error.message.clone(),
                expected: error.expected.clone(),
                observed: error.observed.clone(),
                state_before: scenario.run.traces[error.step_index].state_before.clone(),
                state_after: scenario.run.traces[error.step_index].state_after.clone(),
                input: scenario.run.traces[error.step_index].input.clone(),
                evidence_sha256: scenario.run.traces[error.step_index]
                    .evidence
                    .evidence_sha256
                    .clone(),
            })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FirstErrorAt {
    pub scenario: String,
    pub step_index: usize,
    pub step_id: String,
    pub rule: String,
    pub message: String,
    pub expected: String,
    pub observed: String,
    pub state_before: ReplayState,
    pub state_after: ReplayState,
    pub input: RecordedInput,
    pub evidence_sha256: String,
}

fn swap_inputs(
    inputs: &[RecordedInput],
    left: usize,
    right: usize,
) -> Result<(Vec<RecordedInput>, usize, usize), io::Error> {
    if left >= inputs.len() || right >= inputs.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "swap indexes are outside the recovered record",
        ));
    }
    let mut changed = inputs.to_vec();
    changed.swap(left, right);
    Ok((changed, left, right))
}

fn duplicate_submission(
    inputs: &[RecordedInput],
) -> Result<(Vec<RecordedInput>, usize, usize), io::Error> {
    let source_index = inputs
        .iter()
        .position(|input| matches!(input.action, Action::Submit { .. }))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "record has no submit input"))?;
    let inserted_at = source_index + 1;
    let mut duplicate = inputs[source_index].clone();
    duplicate.event_id = format!("{}-duplicate", duplicate.event_id);
    duplicate.sequence += 1;

    let mut changed = inputs.to_vec();
    for input in changed.iter_mut().skip(inserted_at) {
        input.sequence += 1;
    }
    changed.insert(inserted_at, duplicate);
    Ok((changed, source_index, inserted_at))
}

fn compare_runs(observed: &ReplayRun, control: &ReplayRun) -> PrefixComparison {
    let compared_steps = observed.traces.len().min(control.traces.len());
    let mut matched_steps = 0;

    for index in 0..compared_steps {
        let left = &control.traces[index];
        let right = &observed.traces[index];
        let reason = if left.input.event_id != right.input.event_id {
            Some("input_mismatch")
        } else if left.evidence.state_before_sha256 != right.evidence.state_before_sha256 {
            Some("state_mismatch")
        } else if left.evidence.result_sha256 != right.evidence.result_sha256 {
            Some("result_mismatch")
        } else {
            None
        };
        if let Some(reason) = reason {
            return PrefixComparison {
                compared_steps,
                matched_steps,
                first_divergence: Some(divergence(index, left, Some(right), reason)),
            };
        }
        matched_steps += 1;
    }

    let first_divergence = if observed.traces.len() < control.traces.len() {
        let index = observed.traces.len();
        Some(divergence(
            index,
            &control.traces[index],
            None,
            "input_missing",
        ))
    } else if observed.traces.len() > control.traces.len() {
        let index = control.traces.len();
        Some(divergence(
            index,
            &control.traces[index - 1],
            Some(&observed.traces[index]),
            "unexpected_extra_input",
        ))
    } else {
        None
    };
    PrefixComparison {
        compared_steps,
        matched_steps,
        first_divergence,
    }
}

fn divergence(
    index: usize,
    control: &StepTrace,
    observed: Option<&StepTrace>,
    reason: &str,
) -> TraceDivergence {
    TraceDivergence {
        index,
        control_step_id: control.input.event_id.clone(),
        control_action: control.input.action.name().to_owned(),
        observed_step_id: observed.map(|step| step.input.event_id.clone()),
        observed_action: observed.map(|step| step.input.action.name().to_owned()),
        reason: reason.to_owned(),
        control_outcome: outcome_label(control),
        observed_outcome: observed
            .map(outcome_label)
            .unwrap_or_else(|| "absent".to_owned()),
    }
}

fn outcome_label(step: &StepTrace) -> String {
    match &step.outcome {
        crate::model::StepOutcome::Accepted { .. } => "accepted".to_owned(),
        crate::model::StepOutcome::Rejected { rule, .. } => format!("rejected:{rule}"),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ControlSummary {
    pub scenario: String,
    pub steps_executed: usize,
    pub all_accepted: bool,
    pub final_phase: Phase,
    pub evidence_head: String,
    pub trace_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaseSummary {
    pub name: String,
    pub description: String,
    pub mutation: MutationKind,
    pub steps_executed: usize,
    pub accepted_steps: usize,
    pub final_phase: Phase,
    pub first_error: Option<FirstErrorAt>,
    pub comparison_to_control: PrefixComparison,
    pub evidence_head: String,
    pub trace_sha256: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExperimentSummary {
    pub schema: String,
    pub record_id: String,
    pub source_take: String,
    pub recovered_at: String,
    pub media: crate::model::RecordedMedia,
    pub truncation: crate::fixture::TruncationStats,
    pub control: ControlSummary,
    pub first_error_across_scenarios: Option<FirstErrorAt>,
    pub cases: Vec<CaseSummary>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyEvidence {
    pub schema: String,
    pub source_media_sha256: String,
    pub gstreamer_probe_sha256: String,
    pub recovered_record_sha256: String,
    pub truncated_fixture_sha256: String,
    pub truncated_valid_prefix_sha256: String,
    pub control_trace_sha256: String,
    pub case_trace_sha256: BTreeMap<String, String>,
    pub first_error_evidence_sha256: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArtifactManifest {
    pub schema: String,
    pub generated_from: String,
    pub artifacts: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct ArtifactBundle {
    pub root: PathBuf,
    pub manifest: ArtifactManifest,
    pub summary: ExperimentSummary,
}

struct EncodedTrace {
    bytes: Vec<u8>,
    sha256: String,
    evidence_head: String,
}

#[derive(Serialize)]
struct TraceHeader<'a> {
    schema: &'a str,
    kind: &'a str,
    record_id: &'a str,
    scenario: &'a str,
    final_phase: &'a Phase,
    first_error: &'a Option<FirstError>,
    evidence_head: &'a str,
}

fn encode_trace(run: &ReplayRun) -> Result<EncodedTrace, io::Error> {
    let evidence_head = run
        .traces
        .last()
        .map(|trace| trace.evidence.evidence_sha256.as_str())
        .unwrap_or(crate::evidence::GENESIS_EVIDENCE);
    let header = TraceHeader {
        schema: SCHEMA,
        kind: "replay_trace_header",
        record_id: &run.record_id,
        scenario: &run.scenario,
        final_phase: &run.final_state.phase,
        first_error: &run.first_error,
        evidence_head,
    };
    let mut bytes = serde_json::to_vec(&header).map_err(io::Error::other)?;
    bytes.push(b'\n');
    for trace in &run.traces {
        bytes.extend_from_slice(&serde_json::to_vec(trace).map_err(io::Error::other)?);
        bytes.push(b'\n');
    }
    let sha256 = sha256_text(&String::from_utf8_lossy(&bytes));
    Ok(EncodedTrace {
        bytes,
        sha256,
        evidence_head: evidence_head.to_owned(),
    })
}

fn first_error_at(scenario: &Scenario) -> Option<FirstErrorAt> {
    let error = scenario.run.first_error.as_ref()?;
    let trace = scenario.run.traces.get(error.step_index)?;
    Some(FirstErrorAt {
        scenario: scenario.name.clone(),
        step_index: error.step_index,
        step_id: error.step_id.clone(),
        rule: error.rule.clone(),
        message: error.message.clone(),
        expected: error.expected.clone(),
        observed: error.observed.clone(),
        state_before: trace.state_before.clone(),
        state_after: trace.state_after.clone(),
        input: trace.input.clone(),
        evidence_sha256: trace.evidence.evidence_sha256.clone(),
    })
}

fn make_summary(
    experiment: &Experiment,
    control_trace: &EncodedTrace,
    case_traces: &BTreeMap<String, EncodedTrace>,
) -> ExperimentSummary {
    let cases = experiment
        .scenarios
        .iter()
        .map(|scenario| {
            let encoded = &case_traces[&scenario.name];
            CaseSummary {
                name: scenario.name.clone(),
                description: scenario.description.clone(),
                mutation: scenario.mutation.clone(),
                steps_executed: scenario.run.traces.len(),
                accepted_steps: scenario
                    .run
                    .traces
                    .iter()
                    .filter(|trace| {
                        matches!(trace.outcome, crate::model::StepOutcome::Accepted { .. })
                    })
                    .count(),
                final_phase: scenario.run.final_state.phase.clone(),
                first_error: first_error_at(scenario),
                comparison_to_control: scenario.comparison.clone(),
                evidence_head: encoded.evidence_head.clone(),
                trace_sha256: encoded.sha256.clone(),
            }
        })
        .collect();

    ExperimentSummary {
        schema: SCHEMA.to_owned(),
        record_id: experiment.record.record_id.clone(),
        source_take: experiment.record.source_take.clone(),
        recovered_at: experiment.record.recovered_at.clone(),
        media: experiment.record.media.clone(),
        truncation: experiment.truncated.stats.clone(),
        control: ControlSummary {
            scenario: experiment.control.scenario.clone(),
            steps_executed: experiment.control.traces.len(),
            all_accepted: experiment.control.first_error.is_none()
                && experiment.control.final_state.phase == Phase::Verified,
            final_phase: experiment.control.final_state.phase.clone(),
            evidence_head: control_trace.evidence_head.clone(),
            trace_sha256: control_trace.sha256.clone(),
        },
        first_error_across_scenarios: experiment.first_error_across_scenarios(),
        cases,
    }
}

fn make_key_evidence(
    experiment: &Experiment,
    recovered_record_bytes: &[u8],
    summary: &ExperimentSummary,
    control_trace: &EncodedTrace,
    case_traces: &BTreeMap<String, EncodedTrace>,
) -> KeyEvidence {
    KeyEvidence {
        schema: SCHEMA.to_owned(),
        source_media_sha256: experiment.record.media.sha256.clone(),
        gstreamer_probe_sha256: crate::evidence::sha256_text(crate::fixture::GSTREAMER_PROBE_JSON),
        recovered_record_sha256: crate::evidence::sha256_bytes(recovered_record_bytes),
        truncated_fixture_sha256: experiment.truncated.stats.file_sha256.clone(),
        truncated_valid_prefix_sha256: experiment.truncated.stats.valid_prefix_sha256.clone(),
        control_trace_sha256: control_trace.sha256.clone(),
        case_trace_sha256: case_traces
            .iter()
            .map(|(name, trace)| (name.clone(), trace.sha256.clone()))
            .collect(),
        first_error_evidence_sha256: summary
            .first_error_across_scenarios
            .as_ref()
            .map(|error| error.evidence_sha256.clone()),
    }
}

fn render_report(experiment: &Experiment, summary: &ExperimentSummary) -> String {
    let mut out = String::new();
    out.push_str("# 媒体元数据对照实验\n\n");
    out.push_str(&format!(
        "- 恢复记录：`{}`\n- 源 take：`{}`\n- 恢复时间：`{}`\n",
        summary.record_id, summary.source_take, summary.recovered_at
    ));
    out.push_str(&format!(
        "- 媒体：`{}`，{} bytes，SHA-256 `{}`\n",
        summary.media.uri, summary.media.size_bytes, summary.media.sha256
    ));
    out.push_str(&format!(
        "- 截断文件：{} bytes，完整前缀 {} lines / {} inputs / {} bytes，缺失尾部 {} bytes\n\n",
        summary.truncation.total_bytes,
        summary.truncation.complete_lines,
        summary.truncation.complete_inputs,
        summary.truncation.valid_prefix_bytes,
        summary.truncation.incomplete_tail_bytes
    ));

    out.push_str("## 恢复后的重放对照\n\n");
    out.push_str(&format!(
        "恢复控制 `{}` 执行 {} 步，最终状态 `{:?}`，全部接受：{}。\n",
        summary.control.scenario,
        summary.control.steps_executed,
        summary.control.final_phase,
        if summary.control.all_accepted {
            "是"
        } else {
            "否"
        }
    ));
    if let Some(divergence) = &experiment.scenarios[0].comparison.first_divergence {
        out.push_str(&format!(
            "截断前缀的首次分歧在索引 {}：控制记录下一步是 `{}` ({}), 截断记录为 `{}`，原因 `{}`。\n",
            divergence.index,
            divergence.control_step_id,
            divergence.control_action,
            divergence
                .observed_step_id
                .as_deref()
                .unwrap_or("absent"),
            divergence.reason
        ));
    }
    out.push('\n');

    for case in &summary.cases {
        out.push_str(&format!(
            "## 场景：`{}`\n\n{}\n\n",
            case.name, case.description
        ));
        if let Some(error) = &case.first_error {
            out.push_str("**第一个错误状态**\n\n");
            out.push_str(&format!(
                "- 索引 {}，输入 `{}` ({}), 规则 `{}`\n",
                error.step_index,
                error.step_id,
                error.input.action.name(),
                error.rule
            ));
            out.push_str(&format!(
                "- 期望 `{}`，实际 `{}`，说明：{}\n",
                error.expected, error.observed, error.message
            ));
            out.push_str(&format!(
                "- 最后有效状态（进入前）：phase=`{:?}`，cursor={}，revision={}，metadata_sha256=`{}`\n",
                error.state_before.phase,
                error.state_before.cursor,
                error.state_before.revision,
                error.state_before.metadata_sha256
            ));
            out.push_str(&format!(
                "- 首个错误状态（拒绝后）：phase=`{:?}`，cursor={}，revision={}，rule=`{}`\n",
                error.state_after.phase,
                error.state_after.cursor,
                error.state_after.revision,
                error
                    .state_after
                    .first_error
                    .as_ref()
                    .map(|failure| failure.rule.as_str())
                    .unwrap_or("unknown")
            ));
            out.push_str(&format!("- 步骤证据：`{}`\n\n", error.evidence_sha256));
        } else {
            out.push_str("状态机未产生规则错误。\n\n");
        }
        out.push_str(&format!(
            "- 执行 {}/{} 步，接受 {} 步，最终状态 `{:?}`。\n",
            case.steps_executed,
            experiment.record.inputs.len(),
            case.accepted_steps,
            case.final_phase
        ));
        out.push_str(&format!(
            "- 与恢复对照：匹配 {}/{} 步。\n",
            case.comparison_to_control.matched_steps, case.comparison_to_control.compared_steps
        ));
        if let Some(divergence) = &case.comparison_to_control.first_divergence {
            out.push_str(&format!(
                "- 首次分歧：索引 {}，原因 `{}`，控制 `{}`，本场景 `{}`。\n",
                divergence.index,
                divergence.reason,
                divergence.control_step_id,
                divergence.observed_step_id.as_deref().unwrap_or("absent")
            ));
        }
        out.push_str(&format!("- 轨迹 SHA-256：`{}`\n\n", case.trace_sha256));
    }

    out.push_str("## 关键证据\n\n");
    if let Some(error) = &summary.first_error_across_scenarios {
        out.push_str(&format!(
            "跨场景最早错误状态：`{}` 的索引 {}，规则 `{}`，证据 `{}`。\n",
            error.scenario, error.step_index, error.rule, error.evidence_sha256
        ));
    }
    out.push_str(&format!(
        "恢复控制轨迹 SHA-256：`{}`。\n",
        summary.control.trace_sha256
    ));
    out.push_str(&format!(
        "GStreamer 探针快照 SHA-256：`{}`（`gstreamer-probe.json`）。\n",
        crate::evidence::sha256_text(crate::fixture::GSTREAMER_PROBE_JSON)
    ));
    out.push_str(&format!(
        "截断文件完整前缀 SHA-256：`{}`。\n",
        summary.truncation.valid_prefix_sha256
    ));
    out
}

impl Experiment {
    pub fn write_artifacts(&self, root: &Path) -> Result<ArtifactBundle, io::Error> {
        fs::create_dir_all(root)?;
        let record_json = pretty_json(&self.record)?;
        let media_bytes = demo_wave_bytes();
        if crate::evidence::sha256_bytes(&media_bytes) != self.record.media.sha256 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "artifact media bytes do not match the recovered record",
            ));
        }
        write_bytes(root, "recovered-record.json", &record_json)?;
        write_bytes(root, "truncated-record.jsonl", &self.truncated_bytes)?;
        write_bytes(root, "media/take-001.wav", &media_bytes)?;
        write_bytes(
            root,
            "gstreamer-probe.json",
            crate::fixture::GSTREAMER_PROBE_JSON.as_bytes(),
        )?;

        let control_trace = encode_trace(&self.control)?;
        write_bytes(root, "control.trace.jsonl", &control_trace.bytes)?;

        let mut case_traces = BTreeMap::new();
        for scenario in &self.scenarios {
            let encoded = encode_trace(&scenario.run)?;
            write_bytes(
                root,
                &format!("{}.trace.jsonl", scenario.name),
                &encoded.bytes,
            )?;
            case_traces.insert(scenario.name.clone(), encoded);
        }

        let summary = make_summary(self, &control_trace, &case_traces);
        let summary_bytes = pretty_json(&summary)?;
        write_bytes(root, "summary.json", &summary_bytes)?;

        let key_evidence =
            make_key_evidence(self, &record_json, &summary, &control_trace, &case_traces);
        let evidence_bytes = pretty_json(&key_evidence)?;
        write_bytes(root, "key-evidence.json", &evidence_bytes)?;

        let report = render_report(self, &summary);
        write_bytes(root, "report.md", &report.into_bytes())?;

        let mut artifacts = BTreeMap::new();
        for relative in [
            "recovered-record.json",
            "truncated-record.jsonl",
            "media/take-001.wav",
            "gstreamer-probe.json",
            "control.trace.jsonl",
            "summary.json",
            "key-evidence.json",
            "report.md",
        ] {
            artifacts.insert(
                relative.to_owned(),
                crate::evidence::file_sha256(&root.join(relative))?,
            );
        }
        for scenario in &self.scenarios {
            let relative = format!("{}.trace.jsonl", scenario.name);
            artifacts.insert(
                relative.clone(),
                crate::evidence::file_sha256(&root.join(relative))?,
            );
        }

        let manifest = ArtifactManifest {
            schema: SCHEMA.to_owned(),
            generated_from: self.record.record_id.clone(),
            artifacts,
        };
        write_bytes(root, "manifest.sha256.json", &pretty_json(&manifest)?)?;

        Ok(ArtifactBundle {
            root: root.to_path_buf(),
            manifest,
            summary,
        })
    }
}

fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>, io::Error> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn validate_recovered_record(record: &RecoveredRecord) -> io::Result<()> {
    let Some(first) = record.inputs.first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "recovered record has no inputs",
        ));
    };
    let Action::Open {
        uri,
        asset_sha256,
        observed_bytes,
    } = &first.action
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "recovered record must begin with an open input",
        ));
    };
    if *uri != record.media.uri
        || *asset_sha256 != record.media.sha256
        || *observed_bytes != record.media.size_bytes
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "open input does not match the recovered media identity",
        ));
    }
    for (index, input) in record.inputs.iter().enumerate() {
        if input.sequence != index as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("recovered input {index} has a non-contiguous sequence"),
            ));
        }
    }
    Ok(())
}

fn write_bytes(root: &Path, relative: &str, bytes: &[u8]) -> io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)
}
