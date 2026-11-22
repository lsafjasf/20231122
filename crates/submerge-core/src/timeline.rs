use crate::conflict::Conflict;
use crate::event::SubtitleEvent;

/// Default silence length that raises a "missing cue" suspicion: 2 seconds.
pub const DEFAULT_GAP_THRESHOLD_NS: u64 = 2_000_000_000;

/// The unified timeline: all events converted to nanoseconds and checked
/// against the video duration.
#[derive(Debug, Clone)]
pub struct Timeline {
    pub video_duration_ns: u64,
    pub gap_threshold_ns: u64,
    events: Vec<SubtitleEvent>,
}

impl Timeline {
    pub fn new(video_duration_ns: u64) -> Self {
        Self {
            video_duration_ns,
            gap_threshold_ns: DEFAULT_GAP_THRESHOLD_NS,
            events: Vec::new(),
        }
    }

    pub fn with_gap_threshold(mut self, threshold_ns: u64) -> Self {
        self.gap_threshold_ns = threshold_ns;
        self
    }

    pub fn add_event(&mut self, event: SubtitleEvent) {
        self.events.push(event);
    }

    pub fn events(&self) -> &[SubtitleEvent] {
        &self.events
    }

    /// Indices of `self.events`, sorted by (unified start, unified end, id).
    fn sorted_indices(&self) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.events.len()).collect();
        order.sort_by_key(|&i| {
            let e = &self.events[i];
            (e.unified_start_ns(), e.unified_end_ns(), e.id)
        });
        order
    }

    /// Detect overlaps, suspicious gaps and out-of-range events.
    pub fn detect_conflicts(&self) -> Vec<Conflict> {
        let mut conflicts = Vec::new();
        let order = self.sorted_indices();

        for &i in &order {
            let e = &self.events[i];
            if e.unified_end_ns() > self.video_duration_ns {
                conflicts.push(Conflict::out_of_range(
                    e.id,
                    e.language,
                    e.unified_end_ns(),
                    self.video_duration_ns,
                ));
            }
        }

        for w in order.windows(2) {
            let prev = &self.events[w[0]];
            let next = &self.events[w[1]];
            let prev_end = prev.unified_end_ns();
            let next_start = next.unified_start_ns();

            if next_start < prev_end {
                conflicts.push(Conflict::overlap(
                    prev.id,
                    prev.language,
                    next.id,
                    next.language,
                    prev_end - next_start,
                ));
            } else {
                let gap = next_start - prev_end;
                if gap > self.gap_threshold_ns {
                    conflicts.push(Conflict::gap(
                        next.id,
                        next.language,
                        gap,
                        self.gap_threshold_ns,
                    ));
                }
            }
        }

        conflicts
    }

    /// Map event id -> list of conflict reasons touching it.
    pub fn reasons_by_event(&self) -> std::collections::HashMap<usize, Vec<String>> {
        let mut map: std::collections::HashMap<usize, Vec<String>> =
            std::collections::HashMap::new();
        for c in self.detect_conflicts() {
            for id in &c.event_ids {
                map.entry(*id).or_default().push(c.to_string());
            }
        }
        map
    }

    /// Partially reorder a selection of events.
    ///
    /// `selection` lists event ids in their current order; `new_order` is a
    /// permutation of positions, e.g. `[2, 0, 1]` means the event currently
    /// in position 2 of the selection goes first. The selected events are
    /// re-laid sequentially (keeping each event's duration) starting at the
    /// earliest unified start of the selection. Returns the ids in their new
    /// layout order.
    pub fn reorder_range(
        &mut self,
        selection: &[usize],
        new_order: &[usize],
    ) -> Result<Vec<usize>, String> {
        if selection.len() != new_order.len() {
            return Err("selection and new_order must have the same length".into());
        }
        let mut sorted_new = new_order.to_vec();
        sorted_new.sort_unstable();
        if sorted_new != (0..selection.len()).collect::<Vec<_>>() {
            return Err("new_order must be a permutation of 0..selection.len()".into());
        }

        // Collect (id, unified_start, unified_duration) by value to avoid
        // holding references across the later mutation.
        let mut selected: Vec<(usize, u64, u64)> = Vec::with_capacity(selection.len());
        for id in selection {
            match self.events.iter().find(|e| e.id == *id) {
                Some(e) => selected.push((e.id, e.unified_start_ns(), e.unified_duration_ns())),
                None => return Err(format!("unknown event id {}", id)),
            }
        }

        let anchor = selected
            .iter()
            .map(|&(_, start, _)| start)
            .min()
            .expect("non-empty selection");

        // Compute new unified start/end for each selected event, in new order.
        let mut cursor = anchor;
        let mut new_spans: Vec<(usize, u64, u64)> = Vec::with_capacity(selection.len());
        for &pos in new_order {
            let (id, _, dur) = selected[pos];
            new_spans.push((id, cursor, cursor + dur));
            cursor += dur;
        }

        // Write back as frames in each event's own source frame rate.
        for (id, start_ns, end_ns) in new_spans {
            let e = self
                .events
                .iter_mut()
                .find(|e| e.id == id)
                .expect("validated above");
            let fps = e.source_fps;
            e.start_frame = ns_to_frame(start_ns, fps, Round::Floor);
            e.end_frame = ns_to_frame(end_ns, fps, Round::Ceil);
        }

        Ok(new_order.iter().map(|&pos| selected[pos].0).collect())
    }
}

enum Round {
    Floor,
    Ceil,
}

/// Convert unified nanoseconds back to a frame index. Start frames round
/// down, end frames round up, so the event still covers the requested span.
fn ns_to_frame(ns: u64, fps: crate::fps::FrameRate, round: Round) -> u64 {
    let frame_ns = 1_000_000_000u64 * fps.den as u64;
    let num = ns * fps.num as u64;
    match round {
        Round::Floor => num / frame_ns,
        Round::Ceil => num.div_ceil(frame_ns),
    }
}
