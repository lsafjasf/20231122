use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{AddressEntry, AddressPolicy, FrameRecord, RecordError, StateView};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SubmitOutcome {
    Accepted {
        object_id: String,
        address: String,
    },
    Duplicate {
        object_id: String,
        address: String,
    },
    Conflict {
        object_id: String,
        existing_address: String,
        incoming_digest: String,
    },
    InvalidRecord {
        category: String,
        detail: String,
    },
}

impl SubmitOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted { .. })
    }
}

#[derive(Debug, Clone)]
pub struct AddressEngine {
    version: String,
    policy: AddressPolicy,
    submission_seq: u64,
    accepted: usize,
    duplicates: usize,
    conflicts: usize,
    invalid: usize,
    entries: BTreeMap<String, AddressEntry>,
    seen_ids: BTreeSet<String>,
}

impl AddressEngine {
    pub fn new(version: impl Into<String>, policy: AddressPolicy) -> Self {
        Self {
            version: version.into(),
            policy,
            submission_seq: 0,
            accepted: 0,
            duplicates: 0,
            conflicts: 0,
            invalid: 0,
            entries: BTreeMap::new(),
            seen_ids: BTreeSet::new(),
        }
    }

    pub fn policy(&self) -> AddressPolicy {
        self.policy
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn state_view(&self) -> StateView {
        StateView {
            version: self.version.clone(),
            policy: Some(self.policy),
            submission_seq: self.submission_seq,
            accepted: self.accepted,
            duplicates: self.duplicates,
            conflicts: self.conflicts,
            invalid: self.invalid,
            entries: self.entries.clone(),
        }
    }

    pub fn submit_raw(&mut self, raw: &str) -> SubmitOutcome {
        match serde_json::from_str::<FrameRecord>(raw) {
            Ok(frame) => self.submit(frame),
            Err(error) => {
                self.submission_seq += 1;
                self.invalid += 1;
                SubmitOutcome::InvalidRecord {
                    category: classify_json_error(&error).to_string(),
                    detail: error.to_string(),
                }
            }
        }
    }

    pub fn submit(&mut self, frame: FrameRecord) -> SubmitOutcome {
        self.submission_seq += 1;
        let incoming_digest = frame.content_digest();

        if let Some(existing) = self.entries.get(&frame.object_id) {
            if existing.content_digest == incoming_digest {
                self.duplicates += 1;
                return SubmitOutcome::Duplicate {
                    object_id: frame.object_id,
                    address: existing.address.clone(),
                };
            }
            self.conflicts += 1;
            return SubmitOutcome::Conflict {
                object_id: frame.object_id,
                existing_address: existing.address.clone(),
                incoming_digest,
            };
        }

        if let Err(error) = frame.validate() {
            self.invalid += 1;
            return SubmitOutcome::InvalidRecord {
                category: match error {
                    RecordError::ChecksumMismatch { .. } => "checksum_mismatch",
                    RecordError::BadChecksumFormat => "checksum_format",
                    _ => "semantic_validation",
                }
                .to_string(),
                detail: error.to_string(),
            };
        }

        let address = match self.policy {
            AddressPolicy::DecodeOrdinal => format!("D{:03}", self.accepted),
            AddressPolicy::PtsStable => format!("P{:010}", frame.pts_ns),
        };
        let entry = AddressEntry {
            object_id: frame.object_id.clone(),
            address: address.clone(),
            frame_kind: frame.frame_kind,
            pts_ns: frame.pts_ns,
            dts_ns: frame.dts_ns,
            submission_index: self.submission_seq - 1,
            content_digest: incoming_digest,
            references: frame.references,
        };
        self.seen_ids.insert(frame.object_id.clone());
        self.entries.insert(frame.object_id.clone(), entry);
        self.accepted += 1;
        SubmitOutcome::Accepted {
            object_id: frame.object_id,
            address,
        }
    }

    pub fn finalize(&mut self) -> FinalizeOutcome {
        self.submission_seq += 1;
        FinalizeOutcome {
            version: self.version.clone(),
            policy: self.policy,
            accepted: self.accepted,
            duplicates: self.duplicates,
            conflicts: self.conflicts,
            invalid: self.invalid,
            state_digest: self.state_view().digest(),
            unique_ids_observed: self.seen_ids.len(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalizeOutcome {
    pub version: String,
    pub policy: AddressPolicy,
    pub accepted: usize,
    pub duplicates: usize,
    pub conflicts: usize,
    pub invalid: usize,
    pub state_digest: String,
    pub unique_ids_observed: usize,
}

fn classify_json_error(error: &serde_json::Error) -> &'static str {
    use serde_json::error::Category;
    match error.classify() {
        Category::Eof => "truncated_json",
        Category::Syntax => "malformed_json",
        Category::Data => "record_schema",
        Category::Io => "io_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FrameKind;

    fn frame(id: &str, kind: FrameKind, pts_ns: u64, dts_ns: u64) -> FrameRecord {
        let payload = format!("{id}:{pts_ns}:{dts_ns}");
        let checksum = crate::model::sha256_hex(payload.as_bytes());
        FrameRecord {
            object_id: id.to_string(),
            frame_kind: kind,
            pts_ns,
            dts_ns,
            duration_ns: 1_000_000_000,
            references: if kind == FrameKind::I {
                vec![]
            } else {
                vec!["i0".to_string()]
            },
            payload,
            checksum,
        }
    }

    #[test]
    fn ordinals_shift_but_pts_addresses_do_not() {
        let first = frame("b1", FrameKind::B, 1, 2);
        let second = frame("b2", FrameKind::B, 2, 3);

        let mut ordinal = AddressEngine::new("v1", AddressPolicy::DecodeOrdinal);
        assert_eq!(
            ordinal.submit(first.clone()),
            SubmitOutcome::Accepted {
                object_id: "b1".into(),
                address: "D000".into()
            }
        );
        assert_eq!(
            ordinal.submit(second),
            SubmitOutcome::Accepted {
                object_id: "b2".into(),
                address: "D001".into()
            }
        );

        let mut stable = AddressEngine::new("v2", AddressPolicy::PtsStable);
        assert_eq!(
            stable.submit(first),
            SubmitOutcome::Accepted {
                object_id: "b1".into(),
                address: "P0000000001".into()
            }
        );
    }

    #[test]
    fn duplicate_is_idempotent_and_conflict_does_not_mutate() {
        let frame = frame("b1", FrameKind::B, 1, 2);
        let mut engine = AddressEngine::new("v2", AddressPolicy::PtsStable);
        engine.submit(frame.clone());
        let before = engine.state_view();
        assert!(matches!(
            engine.submit(frame.clone()),
            SubmitOutcome::Duplicate { .. }
        ));
        assert_eq!(engine.state_view().entries, before.entries);

        let mut changed = frame;
        changed.payload.push_str("-changed");
        changed.checksum = changed.payload_digest();
        assert!(matches!(
            engine.submit(changed),
            SubmitOutcome::Conflict { .. }
        ));
        assert_eq!(engine.state_view().entries, before.entries);
    }

    #[test]
    fn truncated_json_is_counted_and_next_record_still_works() {
        let mut engine = AddressEngine::new("v2", AddressPolicy::PtsStable);
        let outcome = engine.submit_raw("{\"object_id\":\"b1\",");
        assert!(matches!(
            outcome,
            SubmitOutcome::InvalidRecord { ref category, .. } if category == "truncated_json"
        ));
        let valid = serde_json::to_string(&frame("i0", FrameKind::I, 0, 0)).unwrap();
        assert!(engine.submit_raw(&valid).is_accepted());
        assert_eq!(engine.state_view().accepted, 1);
        assert_eq!(engine.state_view().invalid, 1);
    }
}
