//! Deterministic preview engine.
//!
//! `Engine::apply` is the single state-transition function: identical command
//! histories always produce identical states. Duplicate idempotency keys are
//! detected before any state change.

use crate::journal::{hash_entry, JournalEntry, GENESIS_HASH};
use crate::model::{Command, EngineState, Outcome};
use std::collections::HashMap;

pub struct Engine {
    state: EngineState,
    entries: Vec<JournalEntry>,
    /// idem-key -> seq that first applied it.
    seen_keys: HashMap<String, u64>,
    head_hash: String,
}

impl Default for Engine {
    fn default() -> Self {
        Engine::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            state: EngineState::new(),
            entries: Vec::new(),
            seen_keys: HashMap::new(),
            head_hash: GENESIS_HASH.to_string(),
        }
    }

    pub fn state(&self) -> &EngineState {
        &self.state
    }

    pub fn entries(&self) -> &[JournalEntry] {
        &self.entries
    }

    /// Apply one command with an idempotency key and return the new entry.
    /// Re-submitting the same `idem_key` never mutates state.
    pub fn apply(&mut self, idem_key: impl Into<String>, command: Command) -> &JournalEntry {
        let idem_key = idem_key.into();
        let seq = self.entries.len() as u64;
        let before = self.state.clone();

        let outcome = if let Some(original_seq) = self.seen_keys.get(&idem_key) {
            Outcome::Duplicate {
                original_seq: *original_seq,
            }
        } else {
            self.transition(&command)
        };

        if !matches!(outcome, Outcome::Duplicate { .. }) {
            self.seen_keys.insert(idem_key.clone(), seq);
            self.state.revision += 1;
        }

        let entry_hash = hash_entry(
            seq,
            &idem_key,
            &command,
            &before,
            &self.state,
            &outcome,
            &self.head_hash,
        );
        let entry = JournalEntry {
            seq,
            idem_key,
            command,
            state_before: before,
            state_after: self.state.clone(),
            outcome,
            prev_hash: self.head_hash.clone(),
            entry_hash,
        };
        self.head_hash = entry.entry_hash.clone();
        self.entries.push(entry);
        self.entries.last().unwrap()
    }

    /// Pure transition: may mutate `self.state`; returns the outcome.
    fn transition(&mut self, command: &Command) -> Outcome {
        match command {
            Command::AddClip { clip } => {
                if self.state.find_clip(&clip.id).is_some() {
                    return Outcome::Rejected {
                        reason: format!("clip id '{}' already exists", clip.id),
                    };
                }
                self.state.library.push(clip.clone());
                Outcome::Accepted {
                    detail: format!(
                        "added clip '{}' ({} frames, vfr={})",
                        clip.id,
                        clip.pts_ns.len(),
                        clip.is_vfr()
                    ),
                }
            }
            Command::Place { clip_id, position } => {
                if self.state.find_clip(clip_id).is_none() {
                    return Outcome::Rejected {
                        reason: format!("unknown clip '{clip_id}'"),
                    };
                }
                self.grow_timeline(*position);
                self.state.timeline[*position] = Some(clip_id.clone());
                if !self.state.placed.contains(clip_id) {
                    self.state.placed.push(clip_id.clone());
                }
                Outcome::Accepted {
                    detail: format!("placed '{clip_id}' at position {position}"),
                }
            }
            Command::Remove { clip_id } => match self
                .state
                .timeline
                .iter()
                .position(|slot| slot.as_deref() == Some(clip_id))
            {
                Some(idx) => {
                    self.state.timeline[idx] = None;
                    Outcome::Accepted {
                        detail: format!("removed '{clip_id}' from position {idx}"),
                    }
                }
                None => Outcome::Rejected {
                    reason: format!("'{clip_id}' is not on the timeline"),
                },
            },
            Command::SetRate { fps_num, fps_den } => {
                if *fps_den == 0 || *fps_num == 0 {
                    return Outcome::Rejected {
                        reason: "fps numerator and denominator must be non-zero".into(),
                    };
                }
                self.state.fps_num = *fps_num;
                self.state.fps_den = *fps_den;
                Outcome::Accepted {
                    detail: format!("rate set to {fps_num}/{fps_den} fps"),
                }
            }
            Command::Swap {
                position_a,
                position_b,
            } => {
                let max = position_a.max(position_b);
                self.grow_timeline(*max);
                self.state.timeline.swap(*position_a, *position_b);
                Outcome::Accepted {
                    detail: format!("swapped positions {position_a} and {position_b}"),
                }
            }
            // The unknown extension is deliberately not implemented in the
            // core. It is recorded for evidence and reported as unhandled.
            Command::UnknownExtension { name, .. } => {
                Outcome::UnknownIgnored { name: name.clone() }
            }
        }
    }

    fn grow_timeline(&mut self, position: usize) {
        if position >= self.state.timeline.len() {
            self.state.timeline.resize(position + 1, None);
        }
    }
}
