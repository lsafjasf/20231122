use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Session {
    pub id: String,
    pub checked_at: String,
    pub operator: String,
    pub source: Source,
    pub runs: Vec<Run>,
    pub queue_series: Vec<f64>,
    pub steps: Vec<Step>,
    pub evidence: Vec<Evidence>,
    pub auto_findings: Vec<Finding>,
    pub manual_confirmations: Vec<ManualConfirmation>,
    pub boundary: Boundary,
    pub failure: Failure,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Source {
    pub uri: String,
    pub display: String,
    pub format: String,
    pub video: String,
    pub audio: String,
    pub fingerprint: String,
    pub probed: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Run {
    pub id: String,
    pub label: String,
    pub started: String,
    pub cancel_at: String,
    pub cancel_kind: String,
    pub trigger: String,
    pub actor: String,
    pub last_step: String,
    pub queue_peak: String,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Step {
    pub id: String,
    pub time: String,
    pub name: String,
    pub element: String,
    pub state: String,
    pub detail: String,
    pub input_ref: String,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Evidence {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub source_ref: String,
    pub step_ref: String,
    pub captured: String,
    pub payload: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Finding {
    pub id: String,
    pub severity: String,
    pub confidence: u8,
    pub title: String,
    pub summary: String,
    pub step_refs: Vec<String>,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ManualConfirmation {
    pub id: String,
    pub status: String,
    pub title: String,
    pub author: String,
    pub time: String,
    pub note: String,
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Boundary {
    pub id: String,
    pub window: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub evidence: Vec<String>,
    pub next_action: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Failure {
    pub code: String,
    pub title: String,
    pub detail: String,
    pub command: String,
}

impl Session {
    pub fn load() -> Result<Self, String> {
        serde_json::from_str(include_str!("../data/session.json"))
            .map_err(|error| format!("无法解析检查记录: {error}"))
    }

    pub fn evidence(&self, id: &str) -> Option<&Evidence> {
        self.evidence.iter().find(|item| item.id == id)
    }

    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|item| item.id == id)
    }

    pub fn peak_queue(&self) -> f64 {
        self.queue_series.iter().copied().fold(0.0_f64, f64::max)
    }

    pub fn pending_manual(&self) -> usize {
        self.manual_confirmations
            .iter()
            .filter(|item| item.status == "pending")
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceability_fixture_is_complete() {
        let session = Session::load().expect("fixture should parse");
        assert_eq!(session.runs.len(), 2);
        assert_eq!(session.auto_findings.len(), 3);
        assert_eq!(session.manual_confirmations.len(), 2);
        assert_eq!(session.pending_manual(), 1);
        assert!(!session.boundary.evidence.is_empty());

        for finding in &session.auto_findings {
            for step in &finding.step_refs {
                assert!(session.step(step).is_some(), "missing step {step}");
            }
            for evidence in &finding.evidence {
                assert!(
                    session.evidence(evidence).is_some(),
                    "missing evidence {evidence}"
                );
            }
        }

        for evidence_id in &session.boundary.evidence {
            let evidence = session
                .evidence(evidence_id)
                .expect("boundary evidence should exist");
            assert!(!evidence.payload.is_empty());
            assert!(session.step(&evidence.step_ref).is_some());
        }

        let exported =
            serde_json::to_string_pretty(&session).expect("evidence package should serialize");
        assert!(exported.contains("\"auto_findings\""));
        assert!(exported.contains("\"manual_confirmations\""));
        assert!(exported.contains("\"boundary\""));
    }
}
