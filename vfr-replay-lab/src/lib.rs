//! VFR replay lab: a replayable variable-frame-rate clip-preview experiment.
//!
//! Architecture (each layer is independently testable):
//! - [`engine`], [`journal`], [`replay`], [`model`], [`persist`]: pure Rust
//!   core with zero native dependencies.
//! - [`media`] (feature `gst`): GStreamer 1.28 preview pipeline.
//! - [`appui`] (feature `gtk`): GTK4 shell hosting the pipeline.

pub mod engine;
pub mod journal;
pub mod model;
pub mod persist;
pub mod replay;

#[cfg(feature = "gst")]
pub mod media;

#[cfg(feature = "gtk")]
pub mod appui;

pub use engine::Engine;
pub use journal::{JournalEntry, GENESIS_HASH};
pub use model::{Clip, Command, EngineState, Outcome};
pub use replay::{replay, ReplayReport};

#[cfg(test)]
mod core_tests {
    use super::*;

    fn vfr_clip() -> Clip {
        Clip {
            id: "c1".into(),
            uri: "file:///fixture/c1.webm".into(),
            // 33ms, 33ms, 66ms -> not constant => VFR.
            pts_ns: vec![0, 33_000_000, 66_000_000, 132_000_000],
        }
    }

    #[test]
    fn detects_variable_frame_rate() {
        assert!(vfr_clip().is_vfr());
        let cfr = Clip {
            id: "cfr".into(),
            uri: String::new(),
            pts_ns: vec![0, 33_000_000, 66_000_000, 99_000_000],
        };
        assert!(!cfr.is_vfr());
    }

    #[test]
    fn duplicate_idempotency_key_applies_once() {
        let mut engine = Engine::new();
        let first = engine.apply(
            "k1",
            Command::SetRate {
                fps_num: 60,
                fps_den: 1,
            },
        );
        assert!(matches!(first.outcome, Outcome::Accepted { .. }));
        let second = engine.apply(
            "k1",
            Command::SetRate {
                fps_num: 24,
                fps_den: 1,
            },
        );
        assert!(matches!(second.outcome, Outcome::Duplicate { .. }));
        // The second command's payload must not have taken effect.
        assert_eq!(engine.state().fps_num, 60);
        assert_eq!(engine.entries().len(), 2);
        assert_eq!(engine.state().revision, 1);
    }

    #[test]
    fn swap_changes_order_and_is_replayable() {
        let mut engine = Engine::new();
        for (idx, id) in ["a", "b"].iter().enumerate() {
            engine.apply(
                format!("add-{id}"),
                Command::AddClip {
                    clip: Clip {
                        id: (*id).to_string(),
                        uri: String::new(),
                        pts_ns: vec![0, 33_000_000],
                    },
                },
            );
            engine.apply(
                format!("place-{id}"),
                Command::Place {
                    clip_id: (*id).to_string(),
                    position: idx,
                },
            );
        }
        assert_eq!(
            engine.state().timeline_clip_ids(),
            vec![Some("a".into()), Some("b".into())]
        );
        engine.apply(
            "swap",
            Command::Swap {
                position_a: 0,
                position_b: 1,
            },
        );
        assert_eq!(
            engine.state().timeline_clip_ids(),
            vec![Some("b".into()), Some("a".into())]
        );
        let report = replay(engine.entries()).unwrap();
        assert!(report.is_ok());
        assert_eq!(report.final_state, *engine.state());
    }

    #[test]
    fn unknown_extension_is_recorded_but_ignored() {
        let mut engine = Engine::new();
        let entry = engine.apply(
            "ext-1",
            Command::UnknownExtension {
                name: "x-experimental-twist".into(),
                payload: "42".into(),
            },
        );
        assert!(matches!(entry.outcome, Outcome::UnknownIgnored { .. }));
        assert_eq!(engine.state().revision, 1);
    }

    #[test]
    fn rejects_invalid_rate_and_unknown_clip() {
        let mut engine = Engine::new();
        let bad_rate = engine.apply(
            "r0",
            Command::SetRate {
                fps_num: 0,
                fps_den: 1,
            },
        );
        assert!(matches!(bad_rate.outcome, Outcome::Rejected { .. }));
        let bad_place = engine.apply(
            "p0",
            Command::Place {
                clip_id: "ghost".into(),
                position: 0,
            },
        );
        assert!(matches!(bad_place.outcome, Outcome::Rejected { .. }));
    }
}
