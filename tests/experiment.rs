use std::collections::BTreeMap;

use bframe_addressing_lab::evidence::{run_builtin, scenario, write_evidence};
use bframe_addressing_lab::journal::{StepResult, parse_journal};
use bframe_addressing_lab::model::FrameKind;
use bframe_addressing_lab::replay::replay_entries;

#[test]
fn experiment_is_replayable_and_keeps_required_evidence() {
    let run = run_builtin().expect("experiment runs");
    assert_eq!(run.bundle.fixtures.len(), 2);
    assert_eq!(run.bundle.scenarios.len(), 6);
    assert!(run.bundle.cleanup.temporary_directory_removed);
    assert_eq!(run.bundle.cleanup.residual_entries, 0);

    for scenario in &run.bundle.scenarios {
        let raw = &run.journals[&scenario.id];
        let parsed = parse_journal(raw).expect("journal parses");
        assert_eq!(parsed.len(), scenario.entries.len());
        let replay = replay_entries(&parsed);
        assert!(replay.matched, "{} did not replay: {replay:?}", scenario.id);
        assert!(scenario.entries.iter().all(|entry| {
            entry.input_digest.len() == 64
                && entry.state_before_digest.len() == 64
                && entry.state_after_digest.len() == 64
        }));
    }

    let output = tempfile::tempdir().expect("temporary evidence directory");
    write_evidence(&run, output.path()).expect("evidence writes");
    for name in [
        "experiment.json",
        "key-evidence.json",
        "baseline-v1.jsonl",
        "reordered-v1.jsonl",
        "duplicates-v2.jsonl",
        "truncated-v2.jsonl",
    ] {
        assert!(output.path().join(name).is_file(), "{name} missing");
    }
}

#[test]
fn reorder_shifts_legacy_but_not_pts_addresses() {
    let run = run_builtin().expect("experiment runs");
    let v1_before = &scenario(&run, "baseline-v1")
        .unwrap()
        .summary
        .b_frame_addresses;
    let v1_after = &scenario(&run, "reordered-v1")
        .unwrap()
        .summary
        .b_frame_addresses;
    let v2_before = &scenario(&run, "baseline-v2")
        .unwrap()
        .summary
        .b_frame_addresses;
    let v2_after = &scenario(&run, "reordered-v2")
        .unwrap()
        .summary
        .b_frame_addresses;

    assert_eq!(v1_before["gop0-b1"], "D002");
    assert_eq!(v1_after["gop0-b1"], "D001");
    assert_ne!(v1_before, v1_after);
    assert_eq!(v2_before, v2_after);
    assert_eq!(v2_before["gop0-b1"], "P1000000000");
}

#[test]
fn duplicate_conflict_and_truncation_do_not_stop_the_batch() {
    let run = run_builtin().expect("experiment runs");
    let duplicates = scenario(&run, "duplicates-v2").unwrap();
    assert_eq!(duplicates.summary.accepted, 8);
    assert_eq!(duplicates.summary.duplicates, 1);
    assert_eq!(duplicates.summary.conflicts, 1);
    assert_eq!(duplicates.summary.final_addresses["gop0-b1"], "P1000000000");

    let duplicate_entry = duplicates
        .entries
        .iter()
        .find(|entry| {
            matches!(
                entry.result,
                StepResult::Record {
                    outcome: bframe_addressing_lab::SubmitOutcome::Duplicate { .. }
                }
            )
        })
        .expect("duplicate step");
    assert_eq!(
        duplicate_entry.state_before.entries,
        duplicate_entry.state_after.entries
    );

    let truncated = scenario(&run, "truncated-v2").unwrap();
    assert_eq!(truncated.summary.accepted, 4);
    assert_eq!(truncated.summary.invalid, 1);
    let invalid = truncated
        .entries
        .iter()
        .find(|entry| {
            matches!(
                entry.result,
                StepResult::Record {
                    outcome: bframe_addressing_lab::SubmitOutcome::InvalidRecord { .. }
                }
            )
        })
        .expect("truncated step");
    let invalid_raw = match &invalid.input {
        bframe_addressing_lab::journal::JournalInput::RawSubmitted { raw, .. } => raw,
        _ => panic!("truncated input must retain raw bytes"),
    };
    assert!(!invalid_raw.ends_with('}'));
    assert!(truncated.summary.final_addresses.contains_key("gop0-b1"));
    assert!(truncated.summary.final_addresses.contains_key("gop0-b2"));

    let b_frames: BTreeMap<_, _> = truncated
        .entries
        .last()
        .unwrap()
        .state_after
        .entries
        .iter()
        .filter(|(_, entry)| entry.frame_kind == FrameKind::B)
        .map(|(id, entry)| (id.clone(), entry.address.clone()))
        .collect();
    assert_eq!(b_frames.len(), 2);
}
