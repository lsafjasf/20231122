use submerge_core::{ConflictKind, FrameRate, Language, SubtitleEvent, Timeline};

fn ev(id: usize, lang: Language, start: u64, end: u64, fps: FrameRate) -> SubtitleEvent {
    SubtitleEvent::new(id, lang, format!("cue-{}", id), start, end, fps)
}

/// One minute of video; events sourced from all three frame rates.
fn sample_timeline() -> Timeline {
    let mut tl = Timeline::new(60_000_000_000);
    tl.add_event(ev(1, Language::Zh, 0, 72, FrameRate::FPS_24)); // 0.000 - 3.000
    tl.add_event(ev(2, Language::En, 100, 175, FrameRate::FPS_25)); // 4.000 - 7.000
    tl.add_event(ev(3, Language::Ja, 240, 330, FrameRate::FPS_30)); // 8.000 - 11.000
    tl
}

#[test]
fn converts_each_frame_rate_to_unified_timeline() {
    let tl = sample_timeline();
    let events = tl.events();
    assert_eq!(events[0].unified_start_ns(), 0);
    assert_eq!(events[0].unified_end_ns(), 3_000_000_000);
    assert_eq!(events[1].unified_start_ns(), 4_000_000_000);
    assert_eq!(events[1].unified_end_ns(), 7_000_000_000);
    assert_eq!(events[2].unified_start_ns(), 8_000_000_000);
    assert_eq!(events[2].unified_end_ns(), 11_000_000_000);
}

#[test]
fn flags_two_overlapping_subtitles() {
    let mut tl = Timeline::new(60_000_000_000);
    // 24fps: frames [96,168) = 4.000s - 7.000s
    tl.add_event(ev(1, Language::Zh, 96, 168, FrameRate::FPS_24));
    // 30fps: frames [180,240) = 6.000s - 8.000s -> overlaps 1.0s with #1
    tl.add_event(ev(2, Language::Ja, 180, 240, FrameRate::FPS_30));

    let conflicts = tl.detect_conflicts();
    let overlaps: Vec<_> = conflicts
        .iter()
        .filter(|c| c.kind == ConflictKind::Overlap)
        .collect();
    assert_eq!(overlaps.len(), 1);
    assert_eq!(overlaps[0].event_ids, vec![1, 2]);
    assert!(overlaps[0].reason.contains("1.000s"));
}

#[test]
fn flags_gap_and_out_of_range() {
    let mut tl = Timeline::new(10_000_000_000); // 10s video
    tl.add_event(ev(1, Language::En, 0, 25, FrameRate::FPS_25)); // 0-1s
    tl.add_event(ev(2, Language::En, 150, 200, FrameRate::FPS_25)); // 6-8s, gap of 5s
    tl.add_event(ev(3, Language::Zh, 270, 330, FrameRate::FPS_30)); // 9-11s, out of range

    let conflicts = tl.detect_conflicts();
    assert!(conflicts
        .iter()
        .any(|c| c.kind == ConflictKind::Gap && c.event_ids == vec![2]));
    assert!(conflicts
        .iter()
        .any(|c| c.kind == ConflictKind::OutOfRange && c.event_ids == vec![3]));
}

#[test]
fn partial_reorder_relays_selected_events() {
    let mut tl = Timeline::new(60_000_000_000);
    tl.add_event(ev(1, Language::Zh, 0, 48, FrameRate::FPS_24)); // 0-2s
    tl.add_event(ev(2, Language::En, 50, 125, FrameRate::FPS_25)); // 2-5s
    tl.add_event(ev(3, Language::Ja, 150, 210, FrameRate::FPS_30)); // 5-7s
    tl.add_event(ev(4, Language::Zh, 240, 480, FrameRate::FPS_24)); // 10-20s, untouched

    // Reverse the first three events.
    let new_ids = tl.reorder_range(&[1, 2, 3], &[2, 1, 0]).unwrap();
    assert_eq!(new_ids, vec![3, 2, 1]);

    let get = |id: usize| tl.events().iter().find(|e| e.id == id).unwrap().clone();
    // ja#3 (2s) now first: 0-2s; en#2 (3s): 2-5s; zh#1 (2s): 5-7s.
    assert_eq!(get(3).unified_start_ns(), 0);
    assert_eq!(get(3).unified_end_ns(), 2_000_000_000);
    assert_eq!(get(2).unified_start_ns(), 2_000_000_000);
    assert_eq!(get(2).unified_end_ns(), 5_000_000_000);
    assert_eq!(get(1).unified_start_ns(), 5_000_000_000);
    assert_eq!(get(1).unified_end_ns(), 7_000_000_000);
    // Event 4 untouched.
    assert_eq!(get(4).unified_start_ns(), 10_000_000_000);

    // No overlaps among the reordered events.
    assert!(tl
        .detect_conflicts()
        .iter()
        .all(|c| c.kind != ConflictKind::Overlap));
}

#[test]
fn reorder_rejects_bad_permutation() {
    let mut tl = sample_timeline();
    assert!(tl.reorder_range(&[1, 2], &[0, 0]).is_err());
    assert!(tl.reorder_range(&[1, 2], &[0, 1, 2]).is_err());
    assert!(tl.reorder_range(&[1, 99], &[0, 1]).is_err());
}
