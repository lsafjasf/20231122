use std::collections::HashSet;

use serde_json::json;

use crate::model::{
    BwfSnapshot, FirstError, GstReplayEvidence, MarkerSpec, RunKind, RunReport, SessionState,
    StepPhase, StepRecord, StepResult, StepStatus,
};

pub fn truncated_probe(snapshot: &BwfSnapshot) -> RunReport {
    let mut steps = Vec::new();
    let mut state = SessionState::new("awaiting_asset");
    let mut first_error = None;

    let before = state.clone();
    state.name = "asset_read".to_owned();
    push_step(
        &mut steps,
        StepPhase::Ingest,
        json!({
            "path": snapshot.path,
            "file_len": snapshot.file_len,
            "sha256": snapshot.sha256,
        }),
        before,
        state.clone(),
        StepResult {
            status: StepStatus::Accepted,
            code: "bytes_loaded".to_owned(),
            detail: format!("读取 {} 字节原始记录", snapshot.file_len),
        },
        json!({"sha256": snapshot.sha256}),
    );

    let before = state.clone();
    let index = steps.len() + 1;
    if let Some(fault) = &snapshot.structural_fault {
        state.name = "rejected".to_owned();
        state.first_error_step = Some(index);
        first_error = Some(FirstError {
            step_index: index,
            phase: StepPhase::Parse,
            code: fault.code.clone(),
            message: fault.message.clone(),
            expected: fault.expected.clone(),
            actual: fault.actual.clone(),
            state_before: before.clone(),
            error_state: state.clone(),
        });
        push_step(
            &mut steps,
            StepPhase::Parse,
            json!({
                "declared_data_len": snapshot.data_declared_len,
                "data_payload_offset": snapshot.data_payload_offset,
                "file_len": snapshot.file_len,
            }),
            before,
            state.clone(),
            StepResult {
                status: StepStatus::Rejected,
                code: fault.code.clone(),
                detail: fault.message.clone(),
            },
            json!({
                "byte_offset": fault.byte_offset,
                "expected": fault.expected,
                "actual": fault.actual,
            }),
        );
    } else {
        push_step(
            &mut steps,
            StepPhase::Parse,
            json!({"path": snapshot.path}),
            before.clone(),
            before,
            StepResult {
                status: StepStatus::Warning,
                code: "not_truncated".to_owned(),
                detail: "预期截断记录，但结构检查未发现缺段".to_owned(),
            },
            json!({}),
        );
    }

    RunReport {
        id: "truncated_probe".to_owned(),
        title: "截断记录结构探针".to_owned(),
        kind: RunKind::TruncatedProbe,
        asset: snapshot.path.clone(),
        passed: first_error.is_none(),
        steps,
        first_error,
    }
}

pub fn recovered_control(
    snapshot: &BwfSnapshot,
    replay: &GstReplayEvidence,
    marker_order: &[u32],
) -> RunReport {
    let mut steps = Vec::new();
    let mut state = SessionState::new("awaiting_asset");
    let mut first_error = None;

    ingest_step(&mut steps, &mut state, &snapshot.path, snapshot.file_len);
    let before = state.clone();
    state.name = "parsed".to_owned();
    push_step(
        &mut steps,
        StepPhase::Parse,
        json!({
            "time_reference": snapshot.time_reference_timecode,
            "sample_rate": snapshot.sample_rate,
            "declared_frames": snapshot.declared_frames(),
            "markers": snapshot.markers,
        }),
        before,
        state.clone(),
        StepResult {
            status: StepStatus::Accepted,
            code: "bwf_structure_valid".to_owned(),
            detail: format!(
                "BEXT 基准 {}，Cue 标记 {} 个，data 长度一致",
                snapshot.time_reference_timecode,
                snapshot.markers.len()
            ),
        },
        json!({"structural_fault": snapshot.structural_fault}),
    );

    let before = state.clone();
    let index = steps.len() + 1;
    if replay.passed {
        state.name = "control_replay_passed".to_owned();
        push_step(
            &mut steps,
            StepPhase::GstReplay,
            json!({
                "pipeline": replay.pipeline,
                "asset": replay.asset,
                "expected_frames": replay.expected_frames,
            }),
            before,
            state.clone(),
            StepResult {
                status: StepStatus::Accepted,
                code: "gst_eos_full_duration".to_owned(),
                detail: format!(
                    "GStreamer EOS，解码 {} 帧（期望 {}）",
                    replay.decoded_frames, replay.expected_frames
                ),
            },
            serde_json::to_value(replay).unwrap_or_else(|_| json!({})),
        );
    } else {
        state.name = "rejected".to_owned();
        state.first_error_step = Some(index);
        first_error = Some(FirstError {
            step_index: index,
            phase: StepPhase::GstReplay,
            code: "gst_replay_incomplete".to_owned(),
            message: "恢复后的对照重放未取得完整 EOS/帧数".to_owned(),
            expected: format!("{} 帧且 EOS", replay.expected_frames),
            actual: format!("{} 帧，eos={}", replay.decoded_frames, replay.eos),
            state_before: before.clone(),
            error_state: state.clone(),
        });
        push_step(
            &mut steps,
            StepPhase::GstReplay,
            json!({"pipeline": replay.pipeline}),
            before,
            state.clone(),
            StepResult {
                status: StepStatus::Rejected,
                code: "gst_replay_incomplete".to_owned(),
                detail: "恢复文件未完成全时长重放".to_owned(),
            },
            serde_json::to_value(replay).unwrap_or_else(|_| json!({})),
        );
    }

    submit_markers(
        &mut steps,
        &mut state,
        &mut first_error,
        snapshot,
        marker_order,
    );

    if first_error.is_none() {
        let before = state.clone();
        state.name = "complete".to_owned();
        push_step(
            &mut steps,
            StepPhase::Complete,
            json!({"submitted": marker_order}),
            before,
            state,
            StepResult {
                status: StepStatus::Accepted,
                code: "control_passed".to_owned(),
                detail: "恢复重放与顺序提交全部通过".to_owned(),
            },
            json!({}),
        );
    }

    RunReport {
        id: "recovered_control".to_owned(),
        title: "恢复后重放对照".to_owned(),
        kind: RunKind::RecoveredControl,
        asset: snapshot.path.clone(),
        passed: first_error.is_none(),
        steps,
        first_error,
    }
}

pub fn marker_validation(
    id: &str,
    title: &str,
    kind: RunKind,
    snapshot: &BwfSnapshot,
    control: &RunReport,
    marker_order: &[u32],
) -> RunReport {
    let mut steps = Vec::new();
    let mut state = SessionState::new("awaiting_control_replay");
    let mut first_error = None;

    let before = state.clone();
    state.name = "accepting_markers".to_owned();
    push_step(
        &mut steps,
        StepPhase::Ingest,
        json!({
            "control_run": control.id,
            "control_passed": control.passed,
            "asset": snapshot.path,
        }),
        before,
        state.clone(),
        StepResult {
            status: StepStatus::Accepted,
            code: "control_replay_attached".to_owned(),
            detail: "标记会话绑定到已通过 EOS 的恢复重放".to_owned(),
        },
        json!({"control_first_error": control.first_error}),
    );

    submit_markers(
        &mut steps,
        &mut state,
        &mut first_error,
        snapshot,
        marker_order,
    );

    RunReport {
        id: id.to_owned(),
        title: title.to_owned(),
        kind,
        asset: snapshot.path.clone(),
        passed: first_error.is_none(),
        steps,
        first_error,
    }
}

fn ingest_step(
    steps: &mut Vec<StepRecord>,
    state: &mut SessionState,
    path: &std::path::Path,
    file_len: u64,
) {
    let before = state.clone();
    state.name = "asset_read".to_owned();
    push_step(
        steps,
        StepPhase::Ingest,
        json!({"path": path, "file_len": file_len}),
        before,
        state.clone(),
        StepResult {
            status: StepStatus::Accepted,
            code: "bytes_loaded".to_owned(),
            detail: format!("读取 {file_len} 字节恢复记录"),
        },
        json!({"path": path}),
    );
}

fn submit_markers(
    steps: &mut Vec<StepRecord>,
    state: &mut SessionState,
    first_error: &mut Option<FirstError>,
    snapshot: &BwfSnapshot,
    marker_order: &[u32],
) {
    let mut seen_ids = HashSet::new();
    let mut expected_index = 0_usize;
    let mut last_sample_offset = None;

    for marker_id in marker_order {
        let before = state.clone();
        let index = steps.len() + 1;
        let marker = snapshot.markers.iter().find(|item| item.id == *marker_id);
        let input = json!({
            "marker_id": marker_id,
            "expected_marker": snapshot.markers.get(expected_index).map(|item| item.id),
            "submitted_at_ordinal": index,
        });

        if first_error.is_some() {
            push_step(
                steps,
                StepPhase::SubmitMarker,
                input,
                before.clone(),
                before,
                StepResult {
                    status: StepStatus::ObservedAfterError,
                    code: "session_already_rejected".to_owned(),
                    detail: "仅保留输入记录；首个错误后不再改变状态".to_owned(),
                },
                json!({"first_error_step": first_error.as_ref().map(|error| error.step_index)}),
            );
            continue;
        }

        let Some(marker) = marker else {
            reject(
                steps,
                state,
                first_error,
                index,
                StepPhase::SubmitMarker,
                input,
                before,
                "unknown_marker",
                "提交了不在 BWF Cue 表中的标记",
                "现有 Cue ID",
                format!("marker_id={marker_id}"),
                json!({}),
            );
            continue;
        };

        if seen_ids.contains(&marker.id) {
            reject(
                steps,
                state,
                first_error,
                index,
                StepPhase::SubmitMarker,
                input,
                before,
                "duplicate_marker",
                "同一 Cue 被重复提交",
                "每个 Cue ID 仅出现一次",
                format!("重复 {}", marker.label),
                json!({
                    "marker": marker,
                    "accepted_ids": seen_ids,
                }),
            );
            continue;
        }

        let expected = snapshot.markers.get(expected_index);
        if expected.map(|item| item.id) != Some(marker.id) {
            let expected_text = expected
                .map(|item| format!("{} @ {}", item.label, item.sample_offset))
                .unwrap_or_else(|| "无".to_owned());
            reject(
                steps,
                state,
                first_error,
                index,
                StepPhase::SubmitMarker,
                input,
                before,
                "marker_order_violation",
                "Cue 提交顺序偏离 BWF 样本偏移顺序",
                expected_text,
                format!("{} @ {}", marker.label, marker.sample_offset),
                json!({"marker": marker, "accepted_ids": seen_ids}),
            );
            continue;
        }

        if let Some(last) = last_sample_offset
            && marker.sample_offset <= last
        {
            reject(
                steps,
                state,
                first_error,
                index,
                StepPhase::SubmitMarker,
                input,
                before,
                "timecode_not_monotonic",
                "Cue 的 BWF 样本时间戳未严格递增",
                format!("> {last} samples"),
                format!("{} samples", marker.sample_offset),
                json!({"marker": marker}),
            );
            continue;
        }

        seen_ids.insert(marker.id);
        expected_index += 1;
        last_sample_offset = Some(marker.sample_offset);
        state.accepted_markers = seen_ids.len();
        state.expected_marker_index = expected_index;
        state.last_sample_offset = last_sample_offset;
        push_step(
            steps,
            StepPhase::SubmitMarker,
            json!({
                "marker_id": marker.id,
                "label": marker.label,
                "sample_offset": marker.sample_offset,
                "bwf_timecode": marker.absolute_timecode,
            }),
            before,
            state.clone(),
            StepResult {
                status: StepStatus::Accepted,
                code: "marker_accepted".to_owned(),
                detail: format!(
                    "{} @ {} ({})",
                    marker.label, marker.sample_offset, marker.absolute_timecode
                ),
            },
            json!({"marker": marker, "accepted_ids": seen_ids}),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn reject(
    steps: &mut Vec<StepRecord>,
    state: &mut SessionState,
    first_error: &mut Option<FirstError>,
    index: usize,
    phase: StepPhase,
    input: serde_json::Value,
    before: SessionState,
    code: &str,
    message: &str,
    expected: impl Into<String>,
    actual: impl Into<String>,
    evidence: serde_json::Value,
) {
    let expected = expected.into();
    let actual = actual.into();
    state.name = "rejected".to_owned();
    state.first_error_step = Some(index);
    *first_error = Some(FirstError {
        step_index: index,
        phase: phase.clone(),
        code: code.to_owned(),
        message: message.to_owned(),
        expected: expected.clone(),
        actual: actual.clone(),
        state_before: before.clone(),
        error_state: state.clone(),
    });
    push_step(
        steps,
        phase,
        input,
        before,
        state.clone(),
        StepResult {
            status: StepStatus::Rejected,
            code: code.to_owned(),
            detail: message.to_owned(),
        },
        json!({
            "expected": expected,
            "actual": actual,
            "payload": evidence,
        }),
    );
}

#[allow(clippy::too_many_arguments)]
fn push_step(
    steps: &mut Vec<StepRecord>,
    phase: StepPhase,
    input: serde_json::Value,
    state_before: SessionState,
    state_after: SessionState,
    result: StepResult,
    evidence: serde_json::Value,
) {
    steps.push(StepRecord {
        index: steps.len() + 1,
        phase,
        input,
        state_before,
        state_after,
        result,
        evidence,
    });
}

pub fn marker_ids(markers: &[MarkerSpec]) -> Vec<u32> {
    markers.iter().map(|marker| marker.id).collect()
}
