use crate::input::{InputReadError, InputRecord, JsonlRead};
use crate::model::{Compensation, FrameState, Record, Track, canonical_json};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Idle,
    Streaming,
    Compensating,
    Verified,
    Faulted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorMarker {
    pub rule: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayState {
    pub phase: Phase,
    pub cursor: u64,
    pub accepted: u64,
    pub last_record_id: Option<String>,
    pub last_sequence: Option<u64>,
    pub tracks: BTreeMap<String, Track>,
    pub last_frame_by_track: BTreeMap<String, u64>,
    pub last_pts_by_track: BTreeMap<String, i64>,
    pub seen_record_ids: BTreeSet<String>,
    pub params_fingerprint: Option<String>,
    pub frames_observed: u64,
    pub missing_frames: u64,
    pub compensated_frames: u64,
    pub waveform_digest: String,
    pub first_error: Option<ErrorMarker>,
}

impl Default for ReplayState {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            cursor: 0,
            accepted: 0,
            last_record_id: None,
            last_sequence: None,
            tracks: BTreeMap::new(),
            last_frame_by_track: BTreeMap::new(),
            last_pts_by_track: BTreeMap::new(),
            seen_record_ids: BTreeSet::new(),
            params_fingerprint: None,
            frames_observed: 0,
            missing_frames: 0,
            compensated_frames: 0,
            waveform_digest: waveform_digest(&[]),
            first_error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WavePoint {
    pub sample: u64,
    pub time_ms: u64,
    pub pts_ns: i64,
    pub track_id: String,
    pub track_index: u32,
    pub frame: u64,
    pub source_state: FrameState,
    pub output_luma: f64,
    pub measured_luma: f64,
    pub confidence: f64,
    pub compensation_mode: String,
    pub params_fingerprint: String,
    pub record_id: String,
    #[serde(default)]
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Outcome {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_luma: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StepEvidence {
    pub input_sha256: String,
    pub state_before_sha256: String,
    pub state_after_sha256: String,
    pub result_sha256: String,
    pub previous_evidence_sha256: String,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayStep {
    pub index: u64,
    pub expected_sequence: Option<u64>,
    pub input: Value,
    pub state_before: ReplayState,
    pub outcome: Outcome,
    pub state_after: ReplayState,
    pub evidence: StepEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FirstError {
    pub scenario: String,
    pub step_index: u64,
    pub record_id: Option<String>,
    pub sequence: Option<u64>,
    pub rule: String,
    pub message: String,
    pub expected: String,
    pub observed: String,
    pub state_before: ReplayState,
    pub state_after: ReplayState,
    pub evidence_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayReport {
    pub scenario: String,
    pub description: String,
    pub input_steps: usize,
    pub accepted_steps: usize,
    pub final_phase: Phase,
    pub steps: Vec<ReplayStep>,
    pub waveform: Vec<WavePoint>,
    pub first_error: Option<FirstError>,
}

impl ReplayReport {
    pub fn evidence_tip(&self) -> String {
        self.steps
            .last()
            .map(|step| step.evidence.evidence_sha256.clone())
            .unwrap_or_else(|| "0".repeat(64))
    }
}

pub trait FrameProcessor {
    fn measure_luma(&mut self, point: &WavePoint) -> Result<f64>;
}

#[derive(Default)]
pub struct SyntheticProcessor;

impl FrameProcessor for SyntheticProcessor {
    fn measure_luma(&mut self, point: &WavePoint) -> Result<f64> {
        Ok(point.output_luma)
    }
}

pub fn replay(
    scenario: &str,
    description: &str,
    read: &JsonlRead,
    processor: &mut dyn FrameProcessor,
) -> ReplayReport {
    let mut state = ReplayState::default();
    let mut steps = Vec::new();
    let mut waveform = Vec::new();
    let mut previous = "0".repeat(64);

    for input in &read.records {
        let before_state = state.clone();
        let expected_sequence = Some(before_state.cursor);
        let input_value = input.value.clone();
        let (outcome, next_state, point) =
            process_record(&before_state, input, processor, waveform.len() as u64);
        if let Some(point) = &point {
            waveform.push(point.clone());
        }
        let mut after = next_state;
        after.waveform_digest = waveform_digest(&waveform);
        let evidence = build_evidence(
            &previous,
            &canonical_json(&input_value),
            &canonical_json(&before_state),
            &canonical_json(&after),
            &canonical_json(&outcome),
        );
        if let Some(point) = waveform.last_mut() {
            point.evidence_sha256 = evidence.evidence_sha256.clone();
        }
        let step = ReplayStep {
            index: steps.len() as u64,
            expected_sequence,
            input: input_value,
            state_before: before_state,
            outcome,
            state_after: after.clone(),
            evidence,
        };
        previous = step.evidence.evidence_sha256.clone();
        steps.push(step);
        state = after;
        if state.phase == Phase::Faulted {
            break;
        }
    }

    if state.phase != Phase::Faulted {
        if let Some(error) = &read.error {
            let outcome = Outcome {
                status: "rejected".to_string(),
                effect: None,
                output_luma: None,
                rule: Some(error.rule.to_string()),
                message: Some(truncated_message(error)),
                expected: Some("a complete JSON record terminated by a newline".to_string()),
                observed: Some(truncated_observed(error)),
            };
            let mut after = state.clone();
            after.phase = Phase::Faulted;
            after.first_error = Some(ErrorMarker {
                rule: error.rule.to_string(),
                message: truncated_message(error),
            });
            let input_value = json!({
                "input_kind": if error.rule == "input_truncated" { "truncated" } else { "malformed" },
                "byte_offset": error.byte_offset,
                "raw": error.raw,
                "sha256": error.sha256,
                "parse_error": error.parse_error,
                "complete_prefix_records": error.complete_prefix_records,
            });
            let evidence = build_evidence(
                &previous,
                &canonical_json(&input_value),
                &canonical_json(&state),
                &canonical_json(&after),
                &canonical_json(&outcome),
            );
            steps.push(ReplayStep {
                index: steps.len() as u64,
                expected_sequence: Some(state.cursor),
                input: input_value,
                state_before: state.clone(),
                outcome,
                state_after: after.clone(),
                evidence,
            });
            state = after;
        } else if read.records.is_empty() {
            let outcome = Outcome {
                status: "rejected".to_string(),
                effect: None,
                output_luma: None,
                rule: Some("input_empty".to_string()),
                message: Some("no replay records were found".to_string()),
                expected: Some("at least one complete record".to_string()),
                observed: Some("0 records".to_string()),
            };
            let mut after = state.clone();
            after.phase = Phase::Faulted;
            after.first_error = Some(ErrorMarker {
                rule: "input_empty".to_string(),
                message: "no replay records were found".to_string(),
            });
            let input_value = json!({"input_kind": "empty"});
            let evidence = build_evidence(
                &previous,
                &canonical_json(&input_value),
                &canonical_json(&state),
                &canonical_json(&after),
                &canonical_json(&outcome),
            );
            steps.push(ReplayStep {
                index: 0,
                expected_sequence: Some(0),
                input: input_value,
                state_before: state.clone(),
                outcome,
                state_after: after.clone(),
                evidence,
            });
            state = after;
        } else {
            state.phase = Phase::Verified;
        }
    }

    let first_error = steps
        .iter()
        .find(|step| step.outcome.status == "rejected")
        .map(|step| FirstError {
            scenario: scenario.to_string(),
            step_index: step.index,
            record_id: step
                .input
                .get("record_id")
                .and_then(Value::as_str)
                .map(str::to_string),
            sequence: step.input.get("sequence").and_then(Value::as_u64),
            rule: step.outcome.rule.clone().unwrap_or_default(),
            message: step.outcome.message.clone().unwrap_or_default(),
            expected: step.outcome.expected.clone().unwrap_or_default(),
            observed: step.outcome.observed.clone().unwrap_or_default(),
            state_before: step.state_before.clone(),
            state_after: step.state_after.clone(),
            evidence_sha256: step.evidence.evidence_sha256.clone(),
        });

    ReplayReport {
        scenario: scenario.to_string(),
        description: description.to_string(),
        input_steps: read.records.len() + usize::from(read.error.is_some()),
        accepted_steps: state.accepted as usize,
        final_phase: state.phase,
        steps,
        waveform,
        first_error,
    }
}

fn truncated_message(error: &InputReadError) -> String {
    if error.rule == "input_truncated" {
        format!(
            "record at byte {} is an incomplete JSON object",
            error.byte_offset
        )
    } else {
        format!("record at byte {} is not valid JSON", error.byte_offset)
    }
}

fn truncated_observed(error: &InputReadError) -> String {
    format!(
        "{} retained bytes before EOF; JSON parser: {}",
        error.raw.len(),
        error.parse_error
    )
}

pub fn waveform_digest(points: &[WavePoint]) -> String {
    let canonical: Vec<Value> = points
        .iter()
        .map(|point| {
            json!([
                point.sample,
                point.time_ms,
                point.pts_ns,
                point.track_id,
                point.track_index,
                point.frame,
                point.source_state,
                point.output_luma,
                point.measured_luma,
                point.confidence,
                point.compensation_mode,
                point.params_fingerprint,
                point.record_id
            ])
        })
        .collect();
    crate::model::sha256_hex(&canonical_json(&canonical))
}

pub fn build_evidence(
    previous: &str,
    input: &[u8],
    state_before: &[u8],
    state_after: &[u8],
    result: &[u8],
) -> StepEvidence {
    let input_sha256 = crate::model::sha256_hex(input);
    let state_before_sha256 = crate::model::sha256_hex(state_before);
    let state_after_sha256 = crate::model::sha256_hex(state_after);
    let result_sha256 = crate::model::sha256_hex(result);
    let preimage = json!({
        "previous_evidence_sha256": previous,
        "input_sha256": input_sha256,
        "state_before_sha256": state_before_sha256,
        "state_after_sha256": state_after_sha256,
        "result_sha256": result_sha256,
    });
    StepEvidence {
        input_sha256,
        state_before_sha256,
        state_after_sha256,
        result_sha256,
        previous_evidence_sha256: previous.to_string(),
        evidence_sha256: crate::model::sha256_hex(&canonical_json(&preimage)),
    }
}

fn process_record(
    state: &ReplayState,
    input: &InputRecord,
    processor: &mut dyn FrameProcessor,
    sample: u64,
) -> (Outcome, ReplayState, Option<WavePoint>) {
    let record = match Record::from_value(input.value.clone()) {
        Ok(record) => record,
        Err(failure) => {
            return reject(
                state,
                "record_schema_invalid",
                failure.to_string(),
                "a valid frame-loss-review/record-v2 object",
                format!("record at byte {}", input.byte_offset),
            );
        }
    };

    if record.sequence != state.cursor {
        return reject(
            state,
            "sequence_gap",
            "record sequence does not match the next replay cursor",
            format!("sequence={}", state.cursor),
            format!(
                "sequence={} record_id={} byte_offset={}",
                record.sequence, record.record_id, input.byte_offset
            ),
        );
    }
    if state.seen_record_ids.contains(&record.record_id) {
        return reject(
            state,
            "duplicate_record",
            "record_id was already accepted",
            "a record_id not seen earlier in this replay",
            record.record_id.clone(),
        );
    }
    if let Some(existing) = state.tracks.get(&record.track.id)
        && existing != &record.track
    {
        return reject(
            state,
            "track_changed",
            "track metadata changed during a replay",
            format!("stable metadata for track {}", record.track.id),
            format!("before={existing:?} after={:?}", record.track),
        );
    }
    if let Err(failure) = record.params.validate() {
        return reject(
            state,
            "invalid_params",
            failure.to_string(),
            "valid processing parameters",
            serde_json::to_string(&record.params).unwrap_or_default(),
        );
    }
    let computed_fingerprint = record.params.fingerprint();
    if record.params_fingerprint != computed_fingerprint {
        return reject(
            state,
            "params_fingerprint_mismatch",
            "record parameter fields do not match their declared fingerprint",
            computed_fingerprint,
            record.params_fingerprint.clone(),
        );
    }
    if let Some(previous) = &state.params_fingerprint
        && previous != &record.params_fingerprint
    {
        return reject(
            state,
            "params_changed",
            "processing parameters changed without a new replay session",
            previous.clone(),
            record.params_fingerprint.clone(),
        );
    }
    if let Some(last_frame) = state.last_frame_by_track.get(&record.track.id)
        && record.frame.frame <= *last_frame
    {
        return reject(
            state,
            "frame_regression",
            "frame number did not advance on this track",
            format!("frame > {last_frame}"),
            format!("frame={}", record.frame.frame),
        );
    }
    if let Some(last_pts) = state.last_pts_by_track.get(&record.track.id)
        && record.frame.pts_ns <= *last_pts
    {
        return reject(
            state,
            "pts_regression",
            "presentation timestamp did not advance on this track",
            format!("pts_ns > {last_pts}"),
            format!("pts_ns={}", record.frame.pts_ns),
        );
    }
    let inferred_pts = record.frame.time_ms as i128 * 1_000_000;
    if (record.frame.pts_ns as i128 - inferred_pts).abs() > 500_000 {
        return reject(
            state,
            "pts_time_mismatch",
            "pts_ns and time_ms describe different instants",
            format!("pts_ns={inferred_pts} (±500000 ns)"),
            format!(
                "time_ms={} pts_ns={}",
                record.frame.time_ms, record.frame.pts_ns
            ),
        );
    }

    let (point, effect) = match record.frame.state {
        FrameState::Present => match present_point(&record, sample) {
            Ok(result) => result,
            Err((rule, message, expected, observed)) => {
                return reject(state, rule, message, expected, observed);
            }
        },
        FrameState::Missing => match missing_point(&record, sample) {
            Ok(result) => result,
            Err((rule, message, expected, observed)) => {
                return reject(state, rule, message, expected, observed);
            }
        },
    };
    let measured = match processor.measure_luma(&point) {
        Ok(value) if value.is_finite() && (0.0..=1.0).contains(&value) => value,
        Ok(value) => {
            return reject(
                state,
                "processor_invalid_luma",
                "GStreamer returned an invalid luma sample",
                "a finite luma in [0, 1]",
                value.to_string(),
            );
        }
        Err(failure) => {
            return reject(
                state,
                "processor_failed",
                "GStreamer frame processing failed",
                "one measured luma per accepted record",
                failure.to_string(),
            );
        }
    };
    let point = WavePoint {
        measured_luma: measured,
        ..point
    };
    let outcome = Outcome {
        status: "accepted".to_string(),
        effect: Some(effect),
        output_luma: Some(point.output_luma),
        rule: None,
        message: None,
        expected: None,
        observed: None,
    };
    let mut after = state.clone();
    after.phase = if record.frame.state == FrameState::Missing {
        Phase::Compensating
    } else {
        Phase::Streaming
    };
    after.cursor += 1;
    after.accepted += 1;
    after.last_record_id = Some(record.record_id.clone());
    after.last_sequence = Some(record.sequence);
    after
        .tracks
        .insert(record.track.id.clone(), record.track.clone());
    after
        .last_frame_by_track
        .insert(record.track.id.clone(), record.frame.frame);
    after
        .last_pts_by_track
        .insert(record.track.id.clone(), record.frame.pts_ns);
    after.seen_record_ids.insert(record.record_id);
    after.params_fingerprint = Some(record.params_fingerprint);
    after.frames_observed += 1;
    if record.frame.state == FrameState::Missing {
        after.missing_frames += 1;
        after.compensated_frames += 1;
    }
    (outcome, after, Some(point))
}

type Rejection = (&'static str, String, String, String);

fn present_point(
    record: &Record,
    sample: u64,
) -> std::result::Result<(WavePoint, String), Rejection> {
    let Some(luma) = record.frame.source_luma else {
        return Err((
            "invalid_luma",
            "present frame has no source luma".to_string(),
            "source_luma in [0, 1]".to_string(),
            "missing source_luma".to_string(),
        ));
    };
    if !luma.is_finite() || !(0.0..=1.0).contains(&luma) {
        return Err((
            "invalid_luma",
            "present frame source luma is out of range".to_string(),
            "source_luma in [0, 1]".to_string(),
            luma.to_string(),
        ));
    }
    let effect = format!(
        "present frame accepted; time={} ms track={} frame={} output={:.6}",
        record.frame.time_ms, record.track.id, record.frame.frame, luma
    );
    Ok((
        WavePoint {
            sample,
            time_ms: record.frame.time_ms,
            pts_ns: record.frame.pts_ns,
            track_id: record.track.id.clone(),
            track_index: record.track.index,
            frame: record.frame.frame,
            source_state: FrameState::Present,
            output_luma: luma,
            measured_luma: luma,
            confidence: 1.0,
            compensation_mode: "none".to_string(),
            params_fingerprint: record.params_fingerprint.clone(),
            record_id: record.record_id.clone(),
            evidence_sha256: String::new(),
        },
        effect,
    ))
}

fn missing_point(
    record: &Record,
    sample: u64,
) -> std::result::Result<(WavePoint, String), Rejection> {
    let Some(compensation) = &record.compensation else {
        return Err((
            "missing_compensation",
            "missing frame has no compensation evidence".to_string(),
            "a compensation object with applied=true".to_string(),
            "compensation=null".to_string(),
        ));
    };
    validate_compensation(record, compensation)?;
    let output_luma = compensation.expected_luma();
    if !output_luma.is_finite() || !(0.0..=1.0).contains(&output_luma) {
        return Err((
            "invalid_compensation",
            "compensation output luma is out of range".to_string(),
            "blended luma in [0, 1]".to_string(),
            output_luma.to_string(),
        ));
    }
    if (output_luma - compensation.output_luma).abs() > 1e-6 {
        return Err((
            "compensation_output_mismatch",
            "declared output luma does not match the recorded blend inputs".to_string(),
            format!("{output_luma:.9}"),
            format!("{:.9}", compensation.output_luma),
        ));
    }
    let effect = format!(
        "missing frame compensated by blend (gap {}/{}, confidence {:.3}); time={} ms track={} frame={} output={:.6}",
        compensation.gap_position,
        compensation.gap_length,
        compensation.confidence,
        record.frame.time_ms,
        record.track.id,
        record.frame.frame,
        output_luma
    );
    Ok((
        WavePoint {
            sample,
            time_ms: record.frame.time_ms,
            pts_ns: record.frame.pts_ns,
            track_id: record.track.id.clone(),
            track_index: record.track.index,
            frame: record.frame.frame,
            source_state: FrameState::Missing,
            output_luma,
            measured_luma: output_luma,
            confidence: compensation.confidence,
            compensation_mode: compensation.mode.clone(),
            params_fingerprint: record.params_fingerprint.clone(),
            record_id: record.record_id.clone(),
            evidence_sha256: String::new(),
        },
        effect,
    ))
}

fn validate_compensation(
    record: &Record,
    compensation: &Compensation,
) -> std::result::Result<(), Rejection> {
    if !compensation.applied {
        return Err((
            "missing_compensation",
            "compensation evidence says it was not applied".to_string(),
            "applied=true for a missing frame".to_string(),
            "applied=false".to_string(),
        ));
    }
    if compensation.mode != record.params.mode {
        return Err((
            "compensation_mode_mismatch",
            "compensation mode differs from the processing parameters".to_string(),
            record.params.mode.clone(),
            compensation.mode.clone(),
        ));
    }
    if (compensation.blend - record.params.blend).abs() > 1e-9 {
        return Err((
            "compensation_blend_mismatch",
            "compensation blend differs from the processing parameters".to_string(),
            record.params.blend.to_string(),
            compensation.blend.to_string(),
        ));
    }
    if compensation.gap_length == 0
        || compensation.gap_position == 0
        || compensation.gap_position > compensation.gap_length
    {
        return Err((
            "invalid_compensation",
            "compensation gap position is outside its declared gap".to_string(),
            "0 < gap_position <= gap_length".to_string(),
            format!(
                "gap_position={} gap_length={}",
                compensation.gap_position, compensation.gap_length
            ),
        ));
    }
    if compensation.gap_length > record.params.max_conceal_frames {
        return Err((
            "compensation_limit_exceeded",
            "compensation exceeded the configured concealment limit".to_string(),
            format!("gap_length <= {}", record.params.max_conceal_frames),
            format!("gap_length={}", compensation.gap_length),
        ));
    }
    if !compensation.confidence.is_finite()
        || compensation.confidence < record.params.min_confidence
        || compensation.confidence > 1.0
    {
        return Err((
            "confidence_below_minimum",
            "compensation confidence is outside the accepted range".to_string(),
            format!("confidence >= {}", record.params.min_confidence),
            compensation.confidence.to_string(),
        ));
    }
    let before = &compensation.reference_before;
    let after = &compensation.reference_after;
    if before.frame >= record.frame.frame || after.frame <= record.frame.frame {
        return Err((
            "invalid_reference_frames",
            "compensation references do not straddle the missing frame".to_string(),
            format!("before.frame < {} < after.frame", record.frame.frame),
            format!("before={} after={}", before.frame, after.frame),
        ));
    }
    for reference in [before, after] {
        if !reference.luma.is_finite() || !(0.0..=1.0).contains(&reference.luma) {
            return Err((
                "invalid_reference_luma",
                "compensation reference luma is out of range".to_string(),
                "reference luma in [0, 1]".to_string(),
                format!("frame={} luma={}", reference.frame, reference.luma),
            ));
        }
    }
    Ok(())
}

fn reject(
    state: &ReplayState,
    rule: &'static str,
    message: impl Into<String>,
    expected: impl Into<String>,
    observed: impl Into<String>,
) -> (Outcome, ReplayState, Option<WavePoint>) {
    let message = message.into();
    let expected = expected.into();
    let observed = observed.into();
    let mut after = state.clone();
    after.phase = Phase::Faulted;
    after.first_error = Some(ErrorMarker {
        rule: rule.to_string(),
        message: message.clone(),
    });
    (
        Outcome {
            status: "rejected".to_string(),
            effect: None,
            output_luma: None,
            rule: Some(rule.to_string()),
            message: Some(message),
            expected: Some(expected),
            observed: Some(observed),
        },
        after,
        None,
    )
}
