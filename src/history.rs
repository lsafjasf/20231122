use crate::model::{ClipState, Command, StepSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};

pub fn sha256_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    format!("sha256:{encoded}")
}

pub fn state_sha256(state: &ClipState) -> String {
    sha256_bytes(&serde_json::to_vec(state).expect("clip state is serializable"))
}

pub fn command_sha256(command: &Command) -> String {
    sha256_bytes(&serde_json::to_vec(command).expect("command is serializable"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppliedCommand {
    pub sequence: u64,
    pub submission: Submission,
    pub state_sha256_before: String,
    pub state_sha256_after: String,
    pub before: ClipState,
    pub after: ClipState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Applied,
    Undone,
    Redone,
    DuplicateIgnored,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyResult {
    pub status: StepStatus,
    pub revision: u64,
    pub state_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestRecord {
    fingerprint: String,
    status: StepStatus,
    sequence: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Submission {
    pub request_id: String,
    pub base_revision: Option<u64>,
    pub command: Command,
}

impl From<&StepSpec> for Submission {
    fn from(step: &StepSpec) -> Self {
        Self {
            request_id: step.request_id.clone(),
            base_revision: step.base_revision,
            command: step.command.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct History {
    state: ClipState,
    revision: u64,
    next_sequence: u64,
    undo: Vec<AppliedCommand>,
    redo: Vec<AppliedCommand>,
    requests: BTreeMap<String, RequestRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryError {
    pub code: String,
    pub message: String,
}

impl HistoryError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }
}

impl Display for HistoryError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for HistoryError {}

impl History {
    pub fn new(initial: ClipState) -> Self {
        Self {
            state: initial,
            revision: 0,
            next_sequence: 1,
            undo: Vec::new(),
            redo: Vec::new(),
            requests: BTreeMap::new(),
        }
    }

    pub fn state(&self) -> &ClipState {
        &self.state
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    pub fn requests_len(&self) -> usize {
        self.requests.len()
    }

    pub fn state_hash(&self) -> String {
        state_sha256(&self.state)
    }

    pub fn apply(&mut self, submission: Submission) -> ApplyResult {
        match self.apply_inner(submission) {
            Ok(result) => result,
            Err(error) => self.rejected(error),
        }
    }

    pub fn apply_step(&mut self, step: &StepSpec) -> ApplyResult {
        self.apply(Submission::from(step))
    }

    pub fn undo_entries(&self) -> &[AppliedCommand] {
        &self.undo
    }

    pub fn redo_entries(&self) -> &[AppliedCommand] {
        &self.redo
    }

    fn apply_inner(&mut self, submission: Submission) -> Result<ApplyResult, HistoryError> {
        if submission.request_id.trim().is_empty() {
            return Err(HistoryError::new(
                "empty_request_id",
                "request_id must not be empty",
            ));
        }

        let fingerprint = command_sha256(&submission.command);
        if let Some(record) = self.requests.get(&submission.request_id) {
            if record.fingerprint == fingerprint {
                return Ok(ApplyResult {
                    status: StepStatus::DuplicateIgnored,
                    revision: self.revision,
                    state_sha256: self.state_hash(),
                    sequence: record.sequence,
                    error_code: None,
                    message: Some(format!(
                        "request already processed as {:?}; state unchanged",
                        record.status
                    )),
                });
            }
            return Err(HistoryError::new(
                "request_id_reuse",
                format!(
                    "request_id '{}' was already used for a different command",
                    submission.request_id
                ),
            ));
        }

        let result =
            match submission.command.clone() {
                Command::Trim { start_ms, end_ms } => {
                    self.validate_trim(start_ms, end_ms)?;
                    let mut next = self.state.clone();
                    next.trim_start_ms = start_ms;
                    next.trim_end_ms = end_ms;
                    (StepStatus::Applied, self.commit_edit(&submission, next))
                }
                Command::Gain { gain_milli } => {
                    self.validate_gain(gain_milli)?;
                    let mut next = self.state.clone();
                    next.gain_milli = gain_milli;
                    (StepStatus::Applied, self.commit_edit(&submission, next))
                }
                Command::Undo => {
                    let entry = self.undo.pop().ok_or_else(|| {
                        HistoryError::new("nothing_to_undo", "undo stack is empty")
                    })?;
                    self.state = entry.before.clone();
                    self.revision += 1;
                    let sequence = entry.sequence;
                    self.redo.push(entry);
                    (StepStatus::Undone, sequence)
                }
                Command::Redo => {
                    let entry = self.redo.pop().ok_or_else(|| {
                        HistoryError::new("nothing_to_redo", "redo stack is empty")
                    })?;
                    self.state = entry.after.clone();
                    self.revision += 1;
                    let sequence = entry.sequence;
                    self.undo.push(entry);
                    (StepStatus::Redone, sequence)
                }
            };

        self.requests.insert(
            submission.request_id,
            RequestRecord {
                fingerprint,
                status: result.0.clone(),
                sequence: Some(result.1),
            },
        );

        Ok(ApplyResult {
            status: result.0,
            revision: self.revision,
            state_sha256: self.state_hash(),
            sequence: Some(result.1),
            error_code: None,
            message: None,
        })
    }

    fn commit_edit(&mut self, submission: &Submission, next: ClipState) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let entry = AppliedCommand {
            sequence,
            submission: submission.clone(),
            state_sha256_before: self.state_hash(),
            state_sha256_after: state_sha256(&next),
            before: self.state.clone(),
            after: next.clone(),
        };
        self.state = next;
        self.revision += 1;
        self.undo.push(entry);
        self.redo.clear();
        sequence
    }

    fn validate_trim(&self, start_ms: u32, end_ms: u32) -> Result<(), HistoryError> {
        if start_ms >= end_ms {
            return Err(HistoryError::new(
                "invalid_trim_range",
                format!("trim start {start_ms}ms must be before end {end_ms}ms"),
            ));
        }
        if end_ms > self.state.source.duration_ms {
            return Err(HistoryError::new(
                "trim_out_of_bounds",
                format!(
                    "trim end {end_ms}ms exceeds source duration {}ms",
                    self.state.source.duration_ms
                ),
            ));
        }
        Ok(())
    }

    fn validate_gain(&self, gain_milli: u32) -> Result<(), HistoryError> {
        if !(1..=2000).contains(&gain_milli) {
            return Err(HistoryError::new(
                "invalid_gain",
                format!("gain_milli {gain_milli} is outside 1..=2000"),
            ));
        }
        Ok(())
    }

    fn rejected(&self, error: HistoryError) -> ApplyResult {
        ApplyResult {
            status: StepStatus::Rejected,
            revision: self.revision,
            state_sha256: self.state_hash(),
            sequence: None,
            error_code: Some(error.code),
            message: Some(error.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceSpec;

    fn base() -> ClipState {
        ClipState {
            clip_id: "clip".into(),
            source: SourceSpec {
                frequency_hz: 440,
                duration_ms: 2000,
                sample_rate: 16000,
                channels: 1,
            },
            trim_start_ms: 0,
            trim_end_ms: 2000,
            gain_milli: 1000,
        }
    }

    fn step(id: &str, command: Command) -> StepSpec {
        StepSpec {
            request_id: id.into(),
            base_revision: None,
            command,
        }
    }

    #[test]
    fn commutative_edits_converge_after_reordering() {
        let trim = step(
            "trim",
            Command::Trim {
                start_ms: 200,
                end_ms: 1800,
            },
        );
        let gain = step("gain", Command::Gain { gain_milli: 700 });

        let mut forward = History::new(base());
        forward.apply_step(&trim);
        forward.apply_step(&gain);

        let mut reverse = History::new(base());
        reverse.apply_step(&gain);
        reverse.apply_step(&trim);

        assert_eq!(forward.state(), reverse.state());
        assert_eq!(forward.state_hash(), reverse.state_hash());
        assert_eq!(forward.revision(), 2);
        assert_eq!(reverse.revision(), 2);
    }

    #[test]
    fn duplicate_submission_is_idempotent() {
        let gain = step("gain-once", Command::Gain { gain_milli: 700 });
        let mut history = History::new(base());
        assert_eq!(history.apply_step(&gain).status, StepStatus::Applied);
        let before = history.state_hash();
        let duplicate = history.apply_step(&gain);
        assert_eq!(duplicate.status, StepStatus::DuplicateIgnored);
        assert_eq!(duplicate.revision, 1);
        assert_eq!(duplicate.state_sha256, before);
        assert_eq!(history.undo_len(), 1);
    }
}
