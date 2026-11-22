use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::engine::{AddressEngine, SubmitOutcome};
use crate::journal::{JournalEntry, JournalInput, StepResult, parse_journal, serialize_journal};
use crate::model::{AddressPolicy, FrameRecord, Snapshot, StateView, sha256_hex, stable_json};
use crate::replay::{ReplayCheck, replay_entries};

pub const V1_SNAPSHOT_JSON: &str = include_str!("../fixtures/snapshots/v1.json");
pub const V2_SNAPSHOT_JSON: &str = include_str!("../fixtures/snapshots/v2.json");
pub const TRUNCATED_BATCH: &str = include_str!("../fixtures/batches/truncated-control.jsonl");

const DECODE_ORDER: [&str; 8] = [
    "gop0-i0", "gop0-p3", "gop0-b1", "gop0-b2", "gop1-i4", "gop1-p7", "gop1-b5", "gop1-b6",
];

const PRESENTATION_ORDER: [&str; 8] = [
    "gop0-i0", "gop0-b1", "gop0-b2", "gop0-p3", "gop1-i4", "gop1-b5", "gop1-b6", "gop1-p7",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub passed: bool,
    pub expected: Value,
    pub actual: Value,
    pub note: String,
}

impl Check {
    fn new(
        id: &str,
        passed: bool,
        expected: Value,
        actual: Value,
        note: impl Into<String>,
    ) -> Self {
        Self {
            id: id.to_string(),
            passed,
            expected,
            actual,
            note: note.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioSummary {
    pub accepted: usize,
    pub duplicates: usize,
    pub conflicts: usize,
    pub invalid: usize,
    pub final_addresses: BTreeMap<String, String>,
    pub b_frame_addresses: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioReport {
    pub id: String,
    pub title: String,
    pub version: String,
    pub order: String,
    pub summary: ScenarioSummary,
    pub entries: Vec<JournalEntry>,
    pub journal_digest: String,
    pub replay: ReplayCheck,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupReport {
    pub staged_files: usize,
    pub staged_bytes: usize,
    pub temporary_directory_removed: bool,
    pub residual_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotEvidence {
    pub version: String,
    pub label: String,
    pub policy: AddressPolicy,
    pub path: String,
    pub sha256: String,
    pub frame_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExperimentBundle {
    pub schema_version: u32,
    pub experiment: String,
    pub fixtures: BTreeMap<String, SnapshotEvidence>,
    pub scenarios: Vec<ScenarioReport>,
    pub checks: Vec<Check>,
    pub cleanup: CleanupReport,
    pub gstreamer: Option<Value>,
}

pub struct ExperimentRun {
    pub bundle: ExperimentBundle,
    pub journals: BTreeMap<String, String>,
}

pub struct Experiment;

impl Experiment {
    pub fn run_builtin() -> Result<ExperimentRun> {
        let v1: Snapshot = serde_json::from_str(V1_SNAPSHOT_JSON).context("parse v1 snapshot")?;
        let v2: Snapshot = serde_json::from_str(V2_SNAPSHOT_JSON).context("parse v2 snapshot")?;
        validate_snapshot(&v1)?;
        validate_snapshot(&v2)?;

        let mut scenarios = vec![
            run_frames(&v1, &DECODE_ORDER, "baseline-v1", "snapshot order"),
            run_frames(&v2, &DECODE_ORDER, "baseline-v2", "snapshot order"),
            run_frames(
                &v1,
                &PRESENTATION_ORDER,
                "reordered-v1",
                "presentation order",
            ),
            run_frames(
                &v2,
                &PRESENTATION_ORDER,
                "reordered-v2",
                "presentation order",
            ),
            run_duplicates(&v2),
            run_truncated_batch(&v2),
        ];

        let mut journals = BTreeMap::new();
        for scenario in &mut scenarios {
            let raw = serialize_journal(&scenario.entries);
            journals.insert(scenario.id.clone(), raw);
        }

        let (cleanup, replay_checks) = stage_replay_and_cleanup(&mut scenarios, &journals)?;
        let checks = build_checks(&scenarios, &cleanup);
        let fixtures = fixture_evidence(&v1, &v2);

        if let Some(failed) = checks.iter().find(|check| !check.passed) {
            bail!("experiment check failed: {}", failed.id);
        }
        if replay_checks.iter().any(|check| !check.matched) {
            bail!("journal replay mismatch");
        }

        Ok(ExperimentRun {
            bundle: ExperimentBundle {
                schema_version: 1,
                experiment: "B-frame addressing visualization".to_string(),
                fixtures,
                scenarios,
                checks,
                cleanup,
                gstreamer: None,
            },
            journals,
        })
    }

    pub fn to_pretty_json(&self, run: &ExperimentRun) -> Result<String> {
        serde_json::to_string_pretty(&run.bundle).context("serialize experiment bundle")
    }
}

fn validate_snapshot(snapshot: &Snapshot) -> Result<()> {
    if snapshot.schema_version != 1 {
        bail!("unsupported snapshot schema {}", snapshot.schema_version);
    }
    for frame in &snapshot.frames {
        frame
            .validate()
            .with_context(|| format!("invalid snapshot record {}", frame.object_id))?;
    }
    for required in DECODE_ORDER {
        if snapshot.frame(required).is_none() {
            bail!("snapshot {} is missing {required}", snapshot.version);
        }
    }
    Ok(())
}

fn fixture_evidence(v1: &Snapshot, v2: &Snapshot) -> BTreeMap<String, SnapshotEvidence> {
    BTreeMap::from([
        (
            "v1".to_string(),
            SnapshotEvidence {
                version: v1.version.clone(),
                label: v1.label.clone(),
                policy: v1.address_policy,
                path: "fixtures/snapshots/v1.json".to_string(),
                sha256: sha256_hex(V1_SNAPSHOT_JSON.as_bytes()),
                frame_count: v1.frames.len(),
            },
        ),
        (
            "v2".to_string(),
            SnapshotEvidence {
                version: v2.version.clone(),
                label: v2.label.clone(),
                policy: v2.address_policy,
                path: "fixtures/snapshots/v2.json".to_string(),
                sha256: sha256_hex(V2_SNAPSHOT_JSON.as_bytes()),
                frame_count: v2.frames.len(),
            },
        ),
    ])
}

struct ScenarioBuilder {
    id: String,
    title: String,
    version: String,
    order: String,
    engine: Option<AddressEngine>,
    entries: Vec<JournalEntry>,
}

impl ScenarioBuilder {
    fn new(id: &str, title: &str, version: &str, order: &str) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            version: version.to_string(),
            order: order.to_string(),
            engine: None,
            entries: Vec::new(),
        }
    }

    fn load(&mut self, snapshot: &Snapshot, source: &str) {
        let before = self.current_state();
        let engine = AddressEngine::new(snapshot.version.clone(), snapshot.address_policy);
        let after = engine.state_view();
        self.engine = Some(engine);
        self.push(
            format!("load {} snapshot", snapshot.version),
            JournalInput::SnapshotLoaded {
                version: snapshot.version.clone(),
                policy: snapshot.address_policy,
                source: source.to_string(),
            },
            StepResult::SnapshotLoaded {
                version: snapshot.version.clone(),
                policy: snapshot.address_policy,
            },
            before,
            after,
        );
    }

    fn submit(&mut self, frame: FrameRecord, source: &str) {
        let before = self.current_state();
        let outcome = self
            .engine
            .as_mut()
            .expect("scenario must load a snapshot before submitting")
            .submit(frame.clone());
        let after = self.current_state();
        self.push(
            format!("submit {} ({})", frame.object_id, frame.frame_kind.as_str()),
            JournalInput::FrameSubmitted {
                frame,
                source: source.to_string(),
            },
            StepResult::Record { outcome },
            before,
            after,
        );
    }

    fn submit_raw(&mut self, raw: &str, source: &str) {
        let before = self.current_state();
        let outcome = self
            .engine
            .as_mut()
            .expect("scenario must load a snapshot before submitting")
            .submit_raw(raw);
        let after = self.current_state();
        self.push(
            "submit raw record".to_string(),
            JournalInput::RawSubmitted {
                raw: raw.to_string(),
                source: source.to_string(),
            },
            StepResult::Record { outcome },
            before,
            after,
        );
    }

    fn finish(mut self) -> ScenarioReport {
        let before = self.current_state();
        let outcome = self
            .engine
            .as_mut()
            .expect("scenario must load a snapshot")
            .finalize();
        let after = self.current_state();
        self.push(
            "finalize scenario".to_string(),
            JournalInput::Finalized,
            StepResult::Finalized { outcome },
            before,
            after,
        );

        let state = self.current_state();
        let final_addresses = state
            .entries
            .iter()
            .map(|(id, entry)| (id.clone(), entry.address.clone()))
            .collect();
        let journal_digest = sha256_hex(serialize_journal(&self.entries).as_bytes());
        ScenarioReport {
            id: self.id,
            title: self.title,
            version: self.version,
            order: self.order,
            summary: ScenarioSummary {
                accepted: state.accepted,
                duplicates: state.duplicates,
                conflicts: state.conflicts,
                invalid: state.invalid,
                final_addresses,
                b_frame_addresses: state.b_frame_addresses(),
            },
            replay: ReplayCheck {
                checked_entries: self.entries.len(),
                matched: true,
                first_mismatch: None,
            },
            entries: self.entries,
            journal_digest,
        }
    }

    fn current_state(&self) -> StateView {
        self.engine
            .as_ref()
            .map(AddressEngine::state_view)
            .unwrap_or_else(StateView::empty)
    }

    fn push(
        &mut self,
        label: String,
        input: JournalInput,
        result: StepResult,
        before: StateView,
        after: StateView,
    ) {
        let seq = self.entries.len() as u64 + 1;
        self.entries
            .push(JournalEntry::new(seq, label, input, before, result, after));
    }
}

fn run_frames(snapshot: &Snapshot, order: &[&str], id: &str, order_label: &str) -> ScenarioReport {
    let mut builder = ScenarioBuilder::new(
        id,
        &format!("{} / {}", snapshot.version, order_label),
        &snapshot.version,
        order_label,
    );
    builder.load(
        snapshot,
        &format!("fixtures/snapshots/{}.json", snapshot.version),
    );
    for (position, object_id) in order.iter().enumerate() {
        let frame = snapshot
            .frame(object_id)
            .unwrap_or_else(|| panic!("snapshot is missing {object_id}"))
            .clone();
        builder.submit(frame, &format!("{} order index {position}", order_label));
    }
    builder.finish()
}

fn run_duplicates(snapshot: &Snapshot) -> ScenarioReport {
    let mut builder = ScenarioBuilder::new(
        "duplicates-v2",
        "v2 duplicate and conflict submissions",
        &snapshot.version,
        "snapshot order",
    );
    builder.load(snapshot, "fixtures/snapshots/v2.json");
    for object_id in ["gop0-i0", "gop0-p3", "gop0-b1"] {
        builder.submit(
            snapshot.frame(object_id).expect("frame exists").clone(),
            "pre-duplicate prefix",
        );
    }

    let duplicate = snapshot.frame("gop0-b1").expect("frame exists").clone();
    builder.submit(duplicate.clone(), "duplicate submission");

    let mut conflicting = duplicate;
    conflicting.payload = "B-frame|scene=alpha|slot=1|mutated".to_string();
    conflicting.checksum = conflicting.payload_digest();
    builder.submit(conflicting, "conflicting submission");

    for object_id in ["gop0-b2", "gop1-i4", "gop1-p7", "gop1-b5", "gop1-b6"] {
        builder.submit(
            snapshot.frame(object_id).expect("frame exists").clone(),
            "post-duplicate tail",
        );
    }
    builder.finish()
}

fn run_truncated_batch(snapshot: &Snapshot) -> ScenarioReport {
    let mut builder = ScenarioBuilder::new(
        "truncated-v2",
        "v2 truncated control with continued processing",
        &snapshot.version,
        "fixture line order",
    );
    builder.load(snapshot, "fixtures/snapshots/v2.json");
    for (line_index, raw) in TRUNCATED_BATCH.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        builder.submit_raw(
            raw,
            &format!(
                "fixtures/batches/truncated-control.jsonl:{}",
                line_index + 1
            ),
        );
    }
    builder.finish()
}

struct TempWorkspace {
    path: PathBuf,
    staged_files: usize,
    staged_bytes: usize,
}

impl TempWorkspace {
    fn create() -> Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "bframe-addressing-lab-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path)
            .with_context(|| format!("create temporary directory {}", path.display()))?;
        Ok(Self {
            path,
            staged_files: 0,
            staged_bytes: 0,
        })
    }

    fn stage(&mut self, name: &str, contents: &str) -> Result<()> {
        let path = self.path.join(name);
        fs::write(&path, contents).with_context(|| format!("stage journal {}", path.display()))?;
        self.staged_files += 1;
        self.staged_bytes += contents.len();
        Ok(())
    }

    fn load(&self, name: &str) -> Result<String> {
        let path = self.path.join(name);
        fs::read_to_string(&path).with_context(|| format!("read staged journal {}", path.display()))
    }

    fn finish(self) -> Result<CleanupReport> {
        let path = self.path.clone();
        fs::remove_dir_all(&path)
            .with_context(|| format!("remove temporary directory {}", path.display()))?;
        let residual_entries = if path.exists() {
            fs::read_dir(&path)
                .map(|entries| entries.count())
                .unwrap_or(1)
        } else {
            0
        };
        Ok(CleanupReport {
            staged_files: self.staged_files,
            staged_bytes: self.staged_bytes,
            temporary_directory_removed: !path.exists(),
            residual_entries,
        })
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn stage_replay_and_cleanup(
    scenarios: &mut [ScenarioReport],
    journals: &BTreeMap<String, String>,
) -> Result<(CleanupReport, Vec<ReplayCheck>)> {
    let mut workspace = TempWorkspace::create()?;
    for (id, raw) in journals {
        workspace.stage(&format!("{id}.jsonl"), raw)?;
    }

    let mut replay_checks = Vec::new();
    for scenario in scenarios.iter_mut() {
        let raw = workspace.load(&format!("{}.jsonl", scenario.id))?;
        if sha256_hex(raw.as_bytes()) != scenario.journal_digest {
            bail!("journal digest mismatch for {}", scenario.id);
        }
        let parsed =
            parse_journal(&raw).with_context(|| format!("parse staged journal {}", scenario.id))?;
        let check = replay_entries(&parsed);
        scenario.replay = check.clone();
        replay_checks.push(check);
    }

    let cleanup = workspace.finish()?;
    Ok((cleanup, replay_checks))
}

fn build_checks(scenarios: &[ScenarioReport], cleanup: &CleanupReport) -> Vec<Check> {
    let by_id: BTreeMap<&str, &ScenarioReport> = scenarios
        .iter()
        .map(|scenario| (scenario.id.as_str(), scenario))
        .collect();
    let v1_baseline = by_id["baseline-v1"].summary.b_frame_addresses.clone();
    let v1_reordered = by_id["reordered-v1"].summary.b_frame_addresses.clone();
    let v2_baseline = by_id["baseline-v2"].summary.b_frame_addresses.clone();
    let v2_reordered = by_id["reordered-v2"].summary.b_frame_addresses.clone();

    let mut v1_shifted = serde_json::Map::new();
    for (object_id, baseline) in &v1_baseline {
        if let Some(reordered) = v1_reordered.get(object_id) {
            if baseline != reordered {
                v1_shifted.insert(
                    object_id.clone(),
                    json!({ "baseline": baseline, "reordered": reordered }),
                );
            }
        }
    }

    let mut checks = vec![
        Check::new(
            "v1-order-changes-b-addresses",
            v1_baseline != v1_reordered && v1_shifted.contains_key("gop0-b1"),
            json!({ "addresses_equal": false, "gop0-b1_changes": true }),
            json!({
                "addresses_equal": v1_baseline == v1_reordered,
                "changed_b_frames": v1_shifted,
            }),
            "decode-ordinal addresses are tied to submission order",
        ),
        Check::new(
            "v2-order-is-stable",
            v2_baseline == v2_reordered,
            json!({ "addresses_equal": true }),
            json!({
                "addresses_equal": v2_baseline == v2_reordered,
                "baseline": v2_baseline,
                "reordered": v2_reordered,
            }),
            "PTS-derived addresses do not depend on submission order",
        ),
    ];

    let duplicate_scenario = by_id["duplicates-v2"];
    let duplicate_entry = duplicate_scenario.entries.iter().find(|entry| {
        matches!(
            entry.result,
            StepResult::Record {
                outcome: SubmitOutcome::Duplicate { .. }
            }
        )
    });
    let conflict_entry = duplicate_scenario.entries.iter().find(|entry| {
        matches!(
            entry.result,
            StepResult::Record {
                outcome: SubmitOutcome::Conflict { .. }
            }
        )
    });
    let duplicate_idempotent = duplicate_entry
        .map(|entry| {
            entry.state_before.entries == entry.state_after.entries
                && entry.state_before.accepted == entry.state_after.accepted
                && entry.state_after.duplicates == entry.state_before.duplicates + 1
        })
        .unwrap_or(false);
    let conflict_idempotent = conflict_entry
        .map(|entry| {
            entry.state_before.entries == entry.state_after.entries
                && entry.state_before.accepted == entry.state_after.accepted
                && entry.state_after.conflicts == entry.state_before.conflicts + 1
        })
        .unwrap_or(false);
    checks.push(Check::new(
        "duplicate-and-conflict-are-idempotent",
        duplicate_idempotent
            && conflict_idempotent
            && duplicate_scenario.summary.duplicates == 1
            && duplicate_scenario.summary.conflicts == 1
            && duplicate_scenario.summary.accepted == 8,
        json!({
            "duplicate_state_unchanged": true,
            "conflict_state_unchanged": true,
            "duplicates": 1,
            "conflicts": 1,
            "accepted": 8,
        }),
        json!({
            "duplicate_state_unchanged": duplicate_idempotent,
            "conflict_state_unchanged": conflict_idempotent,
            "duplicates": duplicate_scenario.summary.duplicates,
            "conflicts": duplicate_scenario.summary.conflicts,
            "accepted": duplicate_scenario.summary.accepted,
            "duplicate_seq": duplicate_entry.map(|entry| entry.seq),
            "conflict_seq": conflict_entry.map(|entry| entry.seq),
        }),
        "a duplicate is a no-op; a conflicting reuse of object_id is rejected without mutation",
    ));

    let truncated_scenario = by_id["truncated-v2"];
    let invalid_entry = truncated_scenario.entries.iter().find(|entry| {
        matches!(
            entry.result,
            StepResult::Record {
                outcome: SubmitOutcome::InvalidRecord { .. }
            }
        )
    });
    let invalid_category = invalid_entry.and_then(|entry| match &entry.result {
        StepResult::Record {
            outcome: SubmitOutcome::InvalidRecord { category, .. },
        } => Some(category.as_str()),
        _ => None,
    });
    let accepted_after_invalid: Vec<String> = invalid_entry
        .map(|invalid| {
            truncated_scenario
                .entries
                .iter()
                .filter(|entry| entry.seq > invalid.seq)
                .filter_map(|entry| match (&entry.input, &entry.result) {
                    (
                        JournalInput::RawSubmitted { .. },
                        StepResult::Record {
                            outcome: SubmitOutcome::Accepted { object_id, .. },
                        },
                    ) => Some(object_id.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    let continued = accepted_after_invalid == ["gop0-b1".to_string(), "gop0-b2".to_string()];
    checks.push(Check::new(
        "truncated-control-is-isolated",
        invalid_category == Some("truncated_json")
            && truncated_scenario.summary.invalid == 1
            && truncated_scenario.summary.accepted == 4
            && continued,
        json!({
            "invalid_category": "truncated_json",
            "invalid": 1,
            "accepted": 4,
            "accepted_after_invalid": ["gop0-b1", "gop0-b2"],
        }),
        json!({
            "invalid_category": invalid_category,
            "invalid": truncated_scenario.summary.invalid,
            "accepted": truncated_scenario.summary.accepted,
            "accepted_after_invalid": accepted_after_invalid,
            "truncated_seq": invalid_entry.map(|entry| entry.seq),
        }),
        "the truncated JSON line fails alone; the following valid B frames still enter the index",
    ));

    let replay_ok = scenarios.iter().all(|scenario| scenario.replay.matched);
    checks.push(Check::new(
        "journals-replay-exactly",
        replay_ok,
        json!({ "all_scenarios_match": true }),
        json!({
            "all_scenarios_match": replay_ok,
            "scenarios": scenarios.iter().map(|scenario| json!({
                "id": scenario.id,
                "matched": scenario.replay.matched,
                "checked_entries": scenario.replay.checked_entries,
                "journal_digest": scenario.journal_digest,
            })).collect::<Vec<_>>(),
        }),
        "staged JSONL is replayed from disk and every state digest/result is recomputed",
    ));

    let journal_complete = scenarios
        .iter()
        .flat_map(|scenario| &scenario.entries)
        .all(|entry| {
            entry.input_digest.len() == 64
                && entry.state_before_digest.len() == 64
                && entry.state_after_digest.len() == 64
        });
    checks.push(Check::new(
        "every-step-has-input-state-result",
        journal_complete,
        json!({ "complete": true }),
        json!({
            "complete": journal_complete,
            "entry_count": scenarios.iter().map(|scenario| scenario.entries.len()).sum::<usize>(),
        }),
        "each JSONL entry contains input, before-state, result, and after-state digests",
    ));

    checks.push(Check::new(
        "temporary-state-is-cleaned",
        cleanup.temporary_directory_removed && cleanup.residual_entries == 0,
        json!({ "temporary_directory_removed": true, "residual_entries": 0 }),
        stable_json(cleanup),
        "journals are staged and replayed from a temporary directory, then removed",
    ));

    checks
}

pub fn persist_run(run: &ExperimentRun, output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create evidence directory {}", output_dir.display()))?;
    for (id, raw) in &run.journals {
        fs::write(output_dir.join(format!("{id}.jsonl")), raw)
            .with_context(|| format!("write journal {id}.jsonl"))?;
    }
    let bundle =
        serde_json::to_string_pretty(&run.bundle).context("serialize experiment bundle")?;
    fs::write(output_dir.join("experiment.json"), bundle).context("write experiment.json")?;
    fs::write(
        output_dir.join("key-evidence.json"),
        serde_json::to_string_pretty(&json!({
            "checks": run.bundle.checks,
            "fixtures": run.bundle.fixtures,
            "cleanup": run.bundle.cleanup,
            "gstreamer": run.bundle.gstreamer,
        }))
        .context("serialize key evidence")?,
    )
    .context("write key-evidence.json")?;
    Ok(())
}
