//! Append-only, hash-chained experiment journal.
//!
//! Each entry records one input command together with the state *before* and
//! *after* and the outcome. Entries are chained with SHA-256 so any edit to a
//! past entry invalidates every subsequent hash — the replay integrity proof.

use crate::model::{Command, EngineState, Outcome};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// One recorded step of the experiment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// 0-based position in the journal.
    pub seq: u64,
    /// Client-supplied idempotency key; duplicate keys never apply twice.
    pub idem_key: String,
    /// The recorded input.
    pub command: Command,
    /// State snapshot before applying the command.
    pub state_before: EngineState,
    /// State snapshot after applying the command (unchanged on rejection).
    pub state_after: EngineState,
    /// The recorded result.
    pub outcome: Outcome,
    /// Hash of the previous entry (all-zero for the first entry).
    pub prev_hash: String,
    /// Hash covering this entry's full content.
    pub entry_hash: String,
}

pub const GENESIS_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Deterministic hash of the entry content excluding the hash fields.
pub fn hash_entry(
    seq: u64,
    idem_key: &str,
    command: &Command,
    before: &EngineState,
    after: &EngineState,
    outcome: &Outcome,
    prev_hash: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(seq.to_le_bytes());
    hasher.update(idem_key.as_bytes());
    hasher.update(serde_json::to_vec(command).expect("command serializes"));
    hasher.update(serde_json::to_vec(before).expect("state serializes"));
    hasher.update(serde_json::to_vec(after).expect("state serializes"));
    hasher.update(serde_json::to_vec(outcome).expect("outcome serializes"));
    hasher.update(prev_hash.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Verify the hash chain of a loaded journal. Returns the first broken seq.
pub fn verify_chain(entries: &[JournalEntry]) -> Result<(), String> {
    let mut prev = GENESIS_HASH.to_string();
    for entry in entries {
        if entry.prev_hash != prev {
            return Err(format!(
                "seq {} prev_hash mismatch (expected {})",
                entry.seq, prev
            ));
        }
        let recomputed = hash_entry(
            entry.seq,
            &entry.idem_key,
            &entry.command,
            &entry.state_before,
            &entry.state_after,
            &entry.outcome,
            &entry.prev_hash,
        );
        if recomputed != entry.entry_hash {
            return Err(format!("seq {} content hash mismatch", entry.seq));
        }
        prev = entry.entry_hash.clone();
    }
    Ok(())
}
