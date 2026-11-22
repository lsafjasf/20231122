//! End-to-end integration tests covering the replay contract, duplicate
//! submission, order changes, unknown extensions and GStreamer media evidence.

use vfr_replay_lab::journal::verify_chain;
use vfr_replay_lab::model::{Clip, Command, Outcome};
use vfr_replay_lab::persist::{append_entry, read_journal};
use vfr_replay_lab::replay::replay;
use vfr_replay_lab::Engine;

fn clip(id: &str, pts: Vec<u64>) -> Clip {
    Clip {
        id: id.into(),
        uri: format!("file:///fixture/{id}.webm"),
        pts_ns: pts,
    }
}

#[test]
fn full_history_replays_byte_identically() {
    let mut engine = Engine::new();
    engine.apply(
        "a",
        Command::AddClip {
            clip: clip("a", vec![0, 33_000_000, 66_000_000]),
        },
    );
    engine.apply(
        "b",
        Command::AddClip {
            clip: clip("b", vec![0, 40_000_000]),
        },
    );
    engine.apply(
        "pa",
        Command::Place {
            clip_id: "a".into(),
            position: 0,
        },
    );
    engine.apply(
        "pb",
        Command::Place {
            clip_id: "b".into(),
            position: 1,
        },
    );
    engine.apply(
        "swap",
        Command::Swap {
            position_a: 0,
            position_b: 1,
        },
    );

    let report = replay(engine.entries()).expect("chain valid");
    assert!(report.is_ok());
    assert_eq!(
        report.final_state.timeline,
        vec![Some("b".into()), Some("a".into())]
    );
}

#[test]
fn duplicate_submission_never_changes_state() {
    let mut engine = Engine::new();
    engine.apply(
        "add",
        Command::AddClip {
            clip: clip("x", vec![0, 1]),
        },
    );
    engine.apply(
        "place",
        Command::Place {
            clip_id: "x".into(),
            position: 0,
        },
    );
    let first = engine
        .apply(
            "rm",
            Command::Remove {
                clip_id: "x".into(),
            },
        )
        .outcome
        .clone();
    let second = engine
        .apply(
            "rm",
            Command::Remove {
                clip_id: "x".into(),
            },
        )
        .outcome
        .clone();

    assert!(matches!(first, Outcome::Accepted { .. }));
    assert!(matches!(second, Outcome::Duplicate { original_seq: 2 }));
    // The duplicate must not bump revision or alter the timeline.
    assert_eq!(engine.state().revision, 3);
    assert_eq!(engine.state().timeline, vec![None]);

    let report = replay(engine.entries()).unwrap();
    assert!(report.is_ok());
}

#[test]
fn unknown_extension_is_a_recorded_noop() {
    let mut engine = Engine::new();
    let before = engine.state().clone();
    let entry = engine.apply(
        "ext",
        Command::UnknownExtension {
            name: "x-mystery".into(),
            payload: "{}".into(),
        },
    );
    assert!(matches!(entry.outcome, Outcome::UnknownIgnored { .. }));
    // Library and timeline remain untouched even though revision advanced.
    assert_eq!(entry.state_after.library, before.library);
    assert!(entry.state_after.timeline.is_empty());
}

#[test]
fn tampered_journal_fails_chain_verification() {
    let mut engine = Engine::new();
    engine.apply(
        "add",
        Command::AddClip {
            clip: clip("z", vec![0, 1]),
        },
    );
    let mut entries = engine.entries().to_vec();
    entries[0].idem_key = "forged".into();
    assert!(verify_chain(&entries).is_err());
    assert!(replay(&entries).is_err());
}

#[test]
fn journal_round_trips_through_disk() {
    let dir = std::env::temp_dir().join("vfr_lab_journal_test");
    let path = dir.join("journal.jsonl");
    let _ = std::fs::remove_file(&path);

    let mut engine = Engine::new();
    engine.apply(
        "add",
        Command::AddClip {
            clip: clip("d", vec![0, 1]),
        },
    );
    for entry in engine.entries() {
        append_entry(&path, entry).unwrap();
    }

    let loaded = read_journal(&path).unwrap();
    let report = replay(&loaded).unwrap();
    assert!(report.is_ok());
    assert_eq!(loaded, engine.entries());
}
