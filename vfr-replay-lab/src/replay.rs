//! Deterministic replay of a persisted journal.
//!
//! Recovery starts from the genesis state and re-applies every recorded input
//! through the same `Engine::apply`. A replay is successful only when every
//! recomputed state, outcome and entry hash matches the original recording.

use crate::engine::Engine;
use crate::journal::{verify_chain, JournalEntry};
use crate::model::EngineState;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ReplayReport {
    pub steps_replayed: usize,
    pub final_state: EngineState,
    pub chain_ok: bool,
    pub state_divergences: Vec<u64>,
    pub outcome_divergences: Vec<u64>,
}

impl ReplayReport {
    pub fn is_ok(&self) -> bool {
        self.chain_ok && self.state_divergences.is_empty() && self.outcome_divergences.is_empty()
    }
}

/// Replay entries through a fresh engine and compare against the recording.
pub fn replay(entries: &[JournalEntry]) -> Result<ReplayReport, String> {
    verify_chain(entries)?;

    let mut engine = Engine::new();
    let mut state_divergences = Vec::new();
    let mut outcome_divergences = Vec::new();

    for recorded in entries {
        let replayed = engine.apply(recorded.idem_key.clone(), recorded.command.clone());
        if replayed.state_after != recorded.state_after
            || replayed.state_before != recorded.state_before
        {
            state_divergences.push(recorded.seq);
        }
        if replayed.outcome != recorded.outcome {
            outcome_divergences.push(recorded.seq);
        }
    }

    Ok(ReplayReport {
        steps_replayed: entries.len(),
        final_state: engine.state().clone(),
        chain_ok: true,
        state_divergences,
        outcome_divergences,
    })
}

/// The state an observer would see immediately *after* the entry at `seq`.
pub fn state_at(entries: &[JournalEntry], seq: u64) -> Option<&EngineState> {
    entries
        .iter()
        .find(|e| e.seq == seq)
        .map(|e| &e.state_after)
}
