//! Pairwise keyframe comparison.
//!
//! Two devices pointing at the same repeated object produce separate fragments
//! whose clocks drift apart. After extraction, keyframes already carry a
//! reference-timeline timestamp derived from each device's clock origin and
//! offset. This module matches frames between two segments by nearest
//! reference time within a configurable window, records every pair as
//! independently reviewable evidence, and derives a robust suggested offset
//! for device B from the median of matched deltas.

use crate::model::ids::new_id;
use crate::model::project::{Comparison, ComparisonEntry, Keyframe, MediaSegment};
use crate::time::format_unix_nanos;
use crate::{Error, Result};

/// Default matching window: frames farther apart than this on the reference
/// timeline are not considered the same event.
pub const DEFAULT_MATCH_WINDOW_NS: i64 = 500_000_000;

/// Greedy nearest-neighbour matching on the reference timeline.
///
/// Both keyframe lists are already ordered by PTS/index. Each keyframe of A is
/// paired with the closest still-unpaired keyframe of B when the absolute delta
/// lies within `match_window_ns`. The greedy walk is deterministic and easy to
/// re-check by hand from the exported CSV.
pub fn match_keyframes(
    segment_a: &MediaSegment,
    segment_b: &MediaSegment,
    match_window_ns: i64,
) -> Vec<ComparisonEntry> {
    let mut entries = Vec::new();
    let mut cursor_b = 0usize;
    let mut pair_index = 0u32;

    for keyframe_a in &segment_a.keyframes {
        let start = cursor_b.min(segment_b.keyframes.len());
        let mut best: Option<(usize, i64)> = None;
        for (position, keyframe_b) in segment_b.keyframes.iter().enumerate().skip(start) {
            // B runs on an ordered timeline; once we pass the mirrored time we
            // only need the local neighbourhood.
            let delta = keyframe_b.reference_time_unix_ns - keyframe_a.reference_time_unix_ns;
            let abs_delta = delta.abs();
            if best
                .map(|(_, best_abs)| abs_delta < best_abs)
                .unwrap_or(true)
            {
                best = Some((position, abs_delta));
            }
            if delta > match_window_ns {
                break;
            }
        }

        if let Some((position, abs_delta)) = best {
            if abs_delta <= match_window_ns {
                let keyframe_b = &segment_b.keyframes[position];
                entries.push(build_entry(
                    pair_index, segment_a, segment_b, keyframe_a, keyframe_b,
                ));
                pair_index += 1;
                cursor_b = position + 1;
            }
        }
    }
    entries
}

fn build_entry(
    pair_index: u32,
    segment_a: &MediaSegment,
    segment_b: &MediaSegment,
    keyframe_a: &Keyframe,
    keyframe_b: &Keyframe,
) -> ComparisonEntry {
    let delta_ns = keyframe_b.reference_time_unix_ns - keyframe_a.reference_time_unix_ns;
    ComparisonEntry {
        pair_index,
        segment_a_id: segment_a.id.clone(),
        segment_b_id: segment_b.id.clone(),
        keyframe_a_index: keyframe_a.index,
        keyframe_b_index: keyframe_b.index,
        reference_time_a_unix_ns: keyframe_a.reference_time_unix_ns,
        reference_time_b_unix_ns: keyframe_b.reference_time_unix_ns,
        delta_ns,
        thumbnail_identical: keyframe_a.thumbnail_sha256 == keyframe_b.thumbnail_sha256,
    }
}

/// Median of a slice, without mutating the caller's data.
pub fn median(values: &[i64]) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 1 {
        Some(sorted[mid])
    } else {
        Some((sorted[mid - 1] + sorted[mid]) / 2)
    }
}

/// Build and persist a comparison between two extracted segments.
pub fn compare_segments(
    project: &mut crate::model::project::Project,
    segment_a_id: &str,
    segment_b_id: &str,
    match_window_ns: i64,
    notes: &str,
) -> Result<Comparison> {
    let segment_a = project
        .segment(segment_a_id)
        .ok_or_else(|| Error::SegmentUnknown(segment_a_id.to_string()))?
        .clone();
    let segment_b = project
        .segment(segment_b_id)
        .ok_or_else(|| Error::SegmentUnknown(segment_b_id.to_string()))?
        .clone();
    if segment_a.keyframes.is_empty() || segment_b.keyframes.is_empty() {
        return Err(Error::Validation(
            "both segments must contain keyframes before comparison".into(),
        ));
    }
    if segment_a.group_id != segment_b.group_id {
        return Err(Error::Validation(
            "comparison requires two segments of the same group (repeated object)".into(),
        ));
    }

    let entries = match_keyframes(&segment_a, &segment_b, match_window_ns);
    let deltas: Vec<i64> = entries.iter().map(|entry| entry.delta_ns).collect();
    let median_delta_ns = median(&deltas);
    // To align B onto A, subtract the observed B-A delta from B's clock.
    let suggested_offset_b_ns = median_delta_ns.map(|value| -value);

    let comparison = Comparison {
        id: new_id("cmp"),
        created_at: format_unix_nanos(chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()),
        group_id: segment_a.group_id.clone(),
        segment_a_id: segment_a.id,
        segment_b_id: segment_b.id,
        match_window_ns,
        entries,
        median_delta_ns,
        suggested_offset_b_ns,
        notes: notes.to_string(),
    };
    project.comparisons.push(comparison.clone());
    project.touch();
    Ok(comparison)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::project::Keyframe;

    fn keyframe(index: u32, reference_time: i64, digest: &str) -> Keyframe {
        Keyframe {
            index,
            pts_ns: index as i64 * 1_000_000_000,
            device_time_unix_ns: reference_time,
            reference_time_unix_ns: reference_time,
            is_decoder_keyframe: true,
            thumbnail_sha256: digest.to_string(),
            thumbnail_file: format!("kf-{index}.jpg"),
            thumbnail_bytes: 1,
        }
    }

    fn segment(id: &str, frames: Vec<Keyframe>) -> MediaSegment {
        let mut segment = MediaSegment::new("dev", id, "/tmp/x".into(), 0, "group_1");
        segment.keyframes = frames;
        segment
    }

    #[test]
    fn nearest_neighbour_within_window() {
        let a = segment(
            "a",
            vec![
                keyframe(0, 100, "h0"),
                keyframe(1, 1_000_000_100, "h1"),
                keyframe(2, 2_000_000_100, "h2"),
            ],
        );
        let b = segment(
            "b",
            vec![
                keyframe(0, 150, "g0"),
                keyframe(1, 1_000_000_160, "g1"),
                keyframe(2, 2_000_000_090, "g2"),
            ],
        );
        let entries = match_keyframes(&a, &b, 500);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].delta_ns, 50);
        assert_eq!(entries[1].delta_ns, 60);
        assert_eq!(entries[2].delta_ns, -10);
        assert_eq!(
            median(&entries.iter().map(|e| e.delta_ns).collect::<Vec<_>>()),
            Some(50)
        );
    }

    #[test]
    fn out_of_window_frames_are_skipped() {
        let a = segment("a", vec![keyframe(0, 0, "h0")]);
        let b = segment("b", vec![keyframe(0, 5_000, "g0")]);
        assert!(match_keyframes(&a, &b, 1_000).is_empty());
    }
}
