use std::collections::{HashMap, VecDeque};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_INPUT_FPS: f64 = 1_000_000.0;
const MAX_INPUT_BITRATE_BPS: u64 = 10_000_000_000;
const MAX_INPUT_DROPPED: u64 = 10_000_000_000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodingStats {
    pub encoded_bytes: u64,
    pub frame_rate: f64,
    pub bitrate_bps: u64,
    pub dropped_frames: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncodeSample {
    pub timestamp_ns: u64,
    pub track: u32,
    pub frame: u64,
    pub stats: EncodingStats,
}

impl EncodeSample {
    pub fn new(
        timestamp_ns: u64,
        track: u32,
        frame: u64,
        frame_rate: f64,
        bitrate_bps: u64,
        dropped_frames: u64,
        encoded_bytes: u64,
    ) -> Self {
        Self {
            timestamp_ns,
            track,
            frame,
            stats: EncodingStats {
                encoded_bytes,
                frame_rate,
                bitrate_bps,
                dropped_frames,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricLevel {
    Ok,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Fps,
    Bitrate,
    Dropped,
}

impl Metric {
    pub const ALL: [Metric; 3] = [Metric::Fps, Metric::Bitrate, Metric::Dropped];

    pub fn as_str(self) -> &'static str {
        match self {
            Metric::Fps => "fps",
            Metric::Bitrate => "bitrate",
            Metric::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleFlag {
    NonFiniteFps,
    ClampedFps,
    ClampedBitrate,
    ClampedDropped,
    TimestampRegression,
    TimestampGap,
    FrameRegression,
    FrameGap,
    DropCounterReset,
    EncodedBytesRegression,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessingParams {
    pub window_capacity: usize,
    pub ema_alpha: f64,
    pub reference_fps: f64,
    pub fps_warn: f64,
    pub fps_critical: f64,
    pub reference_bitrate_bps: u64,
    pub bitrate_warn_bps: u64,
    pub bitrate_critical_bps: u64,
    pub drop_warn: u64,
    pub drop_critical: u64,
    pub max_evidence_events: usize,
    pub max_tracks: usize,
}

impl Default for ProcessingParams {
    fn default() -> Self {
        Self {
            window_capacity: 96,
            ema_alpha: 0.35,
            reference_fps: 30.0,
            fps_warn: 25.0,
            fps_critical: 15.0,
            reference_bitrate_bps: 4_000_000,
            bitrate_warn_bps: 5_000_000,
            bitrate_critical_bps: 8_000_000,
            drop_warn: 1,
            drop_critical: 5,
            max_evidence_events: 512,
            max_tracks: 256,
        }
    }
}

impl ProcessingParams {
    pub fn extreme() -> Self {
        Self {
            window_capacity: 8,
            ema_alpha: 0.999,
            reference_fps: 240.0,
            fps_warn: 239.999,
            fps_critical: 120.0,
            reference_bitrate_bps: 1_000_000,
            bitrate_warn_bps: 9_000_000,
            bitrate_critical_bps: 9_999_999,
            drop_warn: 1,
            drop_critical: 2,
            max_evidence_events: 32,
            max_tracks: 8,
        }
    }

    pub fn sanitized(mut self) -> (Self, Vec<String>) {
        let mut changes = Vec::new();

        let capacity = self.window_capacity.clamp(1, 65_536);
        if capacity != self.window_capacity {
            changes.push(format!(
                "window_capacity {} -> {}",
                self.window_capacity, capacity
            ));
            self.window_capacity = capacity;
        }

        let alpha = if self.ema_alpha.is_finite() {
            self.ema_alpha.clamp(0.001, 0.999)
        } else {
            0.35
        };
        if !self.ema_alpha.is_finite() || (alpha - self.ema_alpha).abs() > f64::EPSILON {
            changes.push(format!("ema_alpha {} -> {}", self.ema_alpha, alpha));
            self.ema_alpha = alpha;
        }

        self.reference_fps = sanitize_positive(self.reference_fps, 30.0);
        self.fps_warn = sanitize_positive(self.fps_warn, 25.0).min(self.reference_fps);
        self.fps_critical = sanitize_positive(self.fps_critical, 15.0).min(self.fps_warn);

        self.reference_bitrate_bps = self.reference_bitrate_bps.max(1);
        self.bitrate_warn_bps = self
            .bitrate_warn_bps
            .max(self.reference_bitrate_bps)
            .min(MAX_INPUT_BITRATE_BPS);
        self.bitrate_critical_bps = self
            .bitrate_critical_bps
            .max(self.bitrate_warn_bps)
            .min(MAX_INPUT_BITRATE_BPS);

        self.drop_warn = self.drop_warn.max(1);
        self.drop_critical = self.drop_critical.max(self.drop_warn);
        self.max_evidence_events = self.max_evidence_events.clamp(1, 65_536);
        self.max_tracks = self.max_tracks.clamp(1, 65_536);

        if changes.is_empty() {
            changes.push("none".to_string());
        }
        (self, changes)
    }
}

fn sanitize_positive(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value.min(MAX_INPUT_FPS)
    } else {
        fallback
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveformPoint {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub offset_ms: f64,
    pub track: u32,
    pub frame: u64,
    pub fps: f64,
    pub bitrate_bps: u64,
    pub dropped_delta: u64,
    pub fps_normalized: f64,
    pub bitrate_normalized: f64,
    pub dropped_normalized: f64,
    pub level: MetricLevel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessedPoint {
    pub sequence: u64,
    pub timestamp_ns: u64,
    pub track: u32,
    pub frame: u64,
    pub encoded_bytes: u64,
    pub fps: f64,
    pub bitrate_bps: u64,
    pub dropped_total: u64,
    pub dropped_delta: u64,
    pub fps_ema: f64,
    pub bitrate_ema: f64,
    pub dropped_ema: f64,
    pub flags: Vec<SampleFlag>,
    pub fps_level: MetricLevel,
    pub bitrate_level: MetricLevel,
    pub dropped_level: MetricLevel,
    pub level: MetricLevel,
    pub waveform: WaveformPoint,
}

impl ProcessedPoint {
    pub fn level_for(&self, metric: Metric) -> MetricLevel {
        match metric {
            Metric::Fps => self.fps_level,
            Metric::Bitrate => self.bitrate_level,
            Metric::Dropped => self.dropped_level,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SampleChanges {
    pub timestamp_delta_ns: i128,
    pub frame_delta: u64,
    pub fps_delta: f64,
    pub bitrate_delta_bps: i128,
    pub dropped_delta_change: i128,
    pub level_change: i8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundaryEvent {
    pub event_sequence: u64,
    pub metric: Metric,
    pub from_level: MetricLevel,
    pub to_level: MetricLevel,
    pub before: Option<ProcessedPoint>,
    pub after: ProcessedPoint,
    pub changes: Option<SampleChanges>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatsSnapshot {
    pub has_data: bool,
    pub samples: u64,
    pub rejected_samples: u64,
    pub tracks: usize,
    pub last_track: Option<u32>,
    pub last_frame: Option<u64>,
    pub first_timestamp_ns: Option<u64>,
    pub last_timestamp_ns: Option<u64>,
    pub last_fps: f64,
    pub last_bitrate_bps: u64,
    pub last_encoded_bytes: u64,
    pub last_dropped_total: u64,
    pub last_dropped_delta: u64,
    pub fps_ema: f64,
    pub bitrate_ema: f64,
    pub dropped_ema: f64,
    pub peak_fps: f64,
    pub peak_bitrate_bps: u64,
    pub total_frames: u64,
    pub total_dropped_delta: u64,
    pub anomalies: u64,
    pub warning_samples: u64,
    pub critical_samples: u64,
    pub boundary_events_total: u64,
    pub boundary_events_dropped: u64,
    pub current_level: MetricLevel,
    pub waveform: Vec<WaveformPoint>,
}

impl StatsSnapshot {
    pub fn empty() -> Self {
        Self {
            has_data: false,
            samples: 0,
            rejected_samples: 0,
            tracks: 0,
            last_track: None,
            last_frame: None,
            first_timestamp_ns: None,
            last_timestamp_ns: None,
            last_fps: 0.0,
            last_bitrate_bps: 0,
            last_encoded_bytes: 0,
            last_dropped_total: 0,
            last_dropped_delta: 0,
            fps_ema: 0.0,
            bitrate_ema: 0.0,
            dropped_ema: 0.0,
            peak_fps: 0.0,
            peak_bitrate_bps: 0,
            total_frames: 0,
            total_dropped_delta: 0,
            anomalies: 0,
            warning_samples: 0,
            critical_samples: 0,
            boundary_events_total: 0,
            boundary_events_dropped: 0,
            current_level: MetricLevel::Ok,
            waveform: Vec::new(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum IngestError {
    #[error("track limit {limit} reached; track {track} rejected")]
    TrackLimit { limit: usize, track: u32 },
}

#[derive(Debug, Clone)]
struct TrackState {
    last_point: ProcessedPoint,
    levels: [MetricLevel; 3],
}

#[derive(Debug, Clone)]
pub struct StatisticsProcessor {
    params: ProcessingParams,
    points: VecDeque<ProcessedPoint>,
    tracks: HashMap<u32, TrackState>,
    boundaries: Vec<BoundaryEvent>,
    snapshot: StatsSnapshot,
    next_sequence: u64,
    next_event_sequence: u64,
    first_timestamp_ns: Option<u64>,
    ema_initialized: bool,
}

impl StatisticsProcessor {
    pub fn new(params: ProcessingParams) -> Self {
        let (params, _) = params.sanitized();
        Self {
            params,
            points: VecDeque::new(),
            tracks: HashMap::new(),
            boundaries: Vec::new(),
            snapshot: StatsSnapshot::empty(),
            next_sequence: 0,
            next_event_sequence: 0,
            first_timestamp_ns: None,
            ema_initialized: false,
        }
    }

    pub fn params(&self) -> &ProcessingParams {
        &self.params
    }

    pub fn ingest(&mut self, sample: EncodeSample) -> Result<ProcessedPoint, IngestError> {
        let previous = self.tracks.get(&sample.track).cloned();
        if previous.is_none() && self.tracks.len() >= self.params.max_tracks {
            self.snapshot.rejected_samples = self.snapshot.rejected_samples.saturating_add(1);
            return Err(IngestError::TrackLimit {
                limit: self.params.max_tracks,
                track: sample.track,
            });
        }

        let mut flags = Vec::new();
        let fps = clean_fps(sample.stats.frame_rate, &mut flags);
        let bitrate_bps = clean_u64(
            sample.stats.bitrate_bps,
            MAX_INPUT_BITRATE_BPS,
            SampleFlag::ClampedBitrate,
            &mut flags,
        );
        let dropped_total = clean_u64(
            sample.stats.dropped_frames,
            MAX_INPUT_DROPPED,
            SampleFlag::ClampedDropped,
            &mut flags,
        );

        let dropped_delta = if let Some(previous) = previous.as_ref() {
            let point = &previous.last_point;
            let timestamp_delta_ns = sample.timestamp_ns as i128 - point.timestamp_ns as i128;
            if timestamp_delta_ns < 0 {
                flags.push(SampleFlag::TimestampRegression);
            } else if timestamp_delta_ns > 2_000_000_000 {
                flags.push(SampleFlag::TimestampGap);
            }

            if sample.frame >= point.frame {
                if sample.frame - point.frame > 1 {
                    flags.push(SampleFlag::FrameGap);
                }
            } else {
                flags.push(SampleFlag::FrameRegression);
            }

            let dropped_delta = if dropped_total >= point.dropped_total {
                dropped_total - point.dropped_total
            } else {
                flags.push(SampleFlag::DropCounterReset);
                0
            };

            if sample.stats.encoded_bytes < point.encoded_bytes {
                flags.push(SampleFlag::EncodedBytesRegression);
            }
            dropped_delta
        } else {
            dropped_total
        };

        let fps_level = fps_level(fps, &self.params);
        let bitrate_level = bitrate_level(bitrate_bps, &self.params);
        let dropped_level = dropped_level(dropped_delta, &self.params);
        let level = fps_level.max(bitrate_level).max(dropped_level);

        let (fps_ema, bitrate_ema, dropped_ema) = if self.ema_initialized {
            let alpha = self.params.ema_alpha;
            (
                ema(self.snapshot.fps_ema, fps, alpha),
                ema(self.snapshot.bitrate_ema, bitrate_bps as f64, alpha),
                ema(self.snapshot.dropped_ema, dropped_delta as f64, alpha),
            )
        } else {
            self.ema_initialized = true;
            (fps, bitrate_bps as f64, dropped_delta as f64)
        };

        let first_timestamp_ns = *self.first_timestamp_ns.get_or_insert(sample.timestamp_ns);
        let offset_ms = sample.timestamp_ns.saturating_sub(first_timestamp_ns) as f64 / 1_000_000.0;
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);

        let waveform = WaveformPoint {
            sequence,
            timestamp_ns: sample.timestamp_ns,
            offset_ms,
            track: sample.track,
            frame: sample.frame,
            fps,
            bitrate_bps,
            dropped_delta,
            fps_normalized: normalize(fps, self.params.reference_fps),
            bitrate_normalized: normalize(
                bitrate_bps as f64,
                self.params
                    .reference_bitrate_bps
                    .max(self.params.bitrate_critical_bps) as f64,
            ),
            dropped_normalized: normalize(dropped_delta as f64, self.params.drop_critical as f64),
            level,
        };

        let point = ProcessedPoint {
            sequence,
            timestamp_ns: sample.timestamp_ns,
            track: sample.track,
            frame: sample.frame,
            encoded_bytes: sample.stats.encoded_bytes,
            fps,
            bitrate_bps,
            dropped_total,
            dropped_delta,
            fps_ema,
            bitrate_ema,
            dropped_ema,
            flags,
            fps_level,
            bitrate_level,
            dropped_level,
            level,
            waveform,
        };

        if let Some(previous) = previous.as_ref() {
            for metric in Metric::ALL {
                let before_level = previous.levels[metric_index(metric)];
                let after_level = point.level_for(metric);
                if before_level != after_level {
                    self.record_boundary(BoundaryEvent {
                        event_sequence: self.next_event_sequence,
                        metric,
                        from_level: before_level,
                        to_level: after_level,
                        before: Some(previous.last_point.clone()),
                        after: point.clone(),
                        changes: Some(sample_changes(&previous.last_point, &point)),
                    });
                }
            }
        } else {
            for metric in Metric::ALL {
                let after_level = point.level_for(metric);
                if after_level != MetricLevel::Ok {
                    self.record_boundary(BoundaryEvent {
                        event_sequence: self.next_event_sequence,
                        metric,
                        from_level: MetricLevel::Ok,
                        to_level: after_level,
                        before: None,
                        after: point.clone(),
                        changes: None,
                    });
                }
            }
        }

        self.points.push_back(point.clone());
        while self.points.len() > self.params.window_capacity {
            self.points.pop_front();
        }

        let levels = [fps_level, bitrate_level, dropped_level];
        self.tracks.insert(
            sample.track,
            TrackState {
                last_point: point.clone(),
                levels,
            },
        );
        self.update_snapshot(&point);

        Ok(point)
    }

    pub fn snapshot(&self) -> StatsSnapshot {
        let mut snapshot = self.snapshot.clone();
        snapshot.tracks = self.tracks.len();
        snapshot.waveform = self
            .points
            .iter()
            .map(|point| point.waveform.clone())
            .collect();
        snapshot
    }

    pub fn points(&self) -> Vec<ProcessedPoint> {
        self.points.iter().cloned().collect()
    }

    pub fn boundaries(&self) -> &[BoundaryEvent] {
        &self.boundaries
    }

    fn record_boundary(&mut self, mut event: BoundaryEvent) {
        self.snapshot.boundary_events_total = self.snapshot.boundary_events_total.saturating_add(1);
        if self.boundaries.len() < self.params.max_evidence_events {
            event.event_sequence = self.next_event_sequence;
            self.next_event_sequence = self.next_event_sequence.saturating_add(1);
            self.boundaries.push(event);
        } else {
            self.snapshot.boundary_events_dropped =
                self.snapshot.boundary_events_dropped.saturating_add(1);
        }
    }

    fn update_snapshot(&mut self, point: &ProcessedPoint) {
        self.snapshot.has_data = true;
        self.snapshot.samples = self.snapshot.samples.saturating_add(1);
        self.snapshot.total_frames = self.snapshot.total_frames.saturating_add(1);
        self.snapshot.last_track = Some(point.track);
        self.snapshot.last_frame = Some(point.frame);
        self.snapshot
            .first_timestamp_ns
            .get_or_insert(point.timestamp_ns);
        self.snapshot.last_timestamp_ns = Some(point.timestamp_ns);
        self.snapshot.last_fps = point.fps;
        self.snapshot.last_bitrate_bps = point.bitrate_bps;
        self.snapshot.last_encoded_bytes = point.encoded_bytes;
        self.snapshot.last_dropped_total = point.dropped_total;
        self.snapshot.last_dropped_delta = point.dropped_delta;
        self.snapshot.fps_ema = point.fps_ema;
        self.snapshot.bitrate_ema = point.bitrate_ema;
        self.snapshot.dropped_ema = point.dropped_ema;
        self.snapshot.peak_fps = self.snapshot.peak_fps.max(point.fps);
        self.snapshot.peak_bitrate_bps = self.snapshot.peak_bitrate_bps.max(point.bitrate_bps);
        self.snapshot.total_dropped_delta = self
            .snapshot
            .total_dropped_delta
            .saturating_add(point.dropped_delta);
        self.snapshot.current_level = point.level;
        match point.level {
            MetricLevel::Ok => {}
            MetricLevel::Warning => {
                self.snapshot.warning_samples = self.snapshot.warning_samples.saturating_add(1);
                self.snapshot.anomalies = self.snapshot.anomalies.saturating_add(1);
            }
            MetricLevel::Critical => {
                self.snapshot.critical_samples = self.snapshot.critical_samples.saturating_add(1);
                self.snapshot.anomalies = self.snapshot.anomalies.saturating_add(1);
            }
        }
    }
}

fn clean_fps(value: f64, flags: &mut Vec<SampleFlag>) -> f64 {
    if !value.is_finite() {
        flags.push(SampleFlag::NonFiniteFps);
        return 0.0;
    }
    if value < 0.0 {
        flags.push(SampleFlag::ClampedFps);
        return 0.0;
    }
    if value > MAX_INPUT_FPS {
        flags.push(SampleFlag::ClampedFps);
        return MAX_INPUT_FPS;
    }
    value
}

fn clean_u64(value: u64, max: u64, flag: SampleFlag, flags: &mut Vec<SampleFlag>) -> u64 {
    if value > max {
        flags.push(flag);
        max
    } else {
        value
    }
}

fn fps_level(value: f64, params: &ProcessingParams) -> MetricLevel {
    if value < params.fps_critical {
        MetricLevel::Critical
    } else if value < params.fps_warn {
        MetricLevel::Warning
    } else {
        MetricLevel::Ok
    }
}

fn bitrate_level(value: u64, params: &ProcessingParams) -> MetricLevel {
    if value > params.bitrate_critical_bps {
        MetricLevel::Critical
    } else if value > params.bitrate_warn_bps {
        MetricLevel::Warning
    } else {
        MetricLevel::Ok
    }
}

fn dropped_level(value: u64, params: &ProcessingParams) -> MetricLevel {
    if value >= params.drop_critical {
        MetricLevel::Critical
    } else if value >= params.drop_warn {
        MetricLevel::Warning
    } else {
        MetricLevel::Ok
    }
}

fn ema(previous: f64, current: f64, alpha: f64) -> f64 {
    alpha * current + (1.0 - alpha) * previous
}

fn normalize(value: f64, reference: f64) -> f64 {
    if !value.is_finite() || reference <= 0.0 {
        0.0
    } else {
        (value / reference).clamp(0.0, 1.0)
    }
}

fn metric_index(metric: Metric) -> usize {
    match metric {
        Metric::Fps => 0,
        Metric::Bitrate => 1,
        Metric::Dropped => 2,
    }
}

fn sample_changes(before: &ProcessedPoint, after: &ProcessedPoint) -> SampleChanges {
    SampleChanges {
        timestamp_delta_ns: after.timestamp_ns as i128 - before.timestamp_ns as i128,
        frame_delta: after.frame.saturating_sub(before.frame),
        fps_delta: after.fps - before.fps,
        bitrate_delta_bps: after.bitrate_bps as i128 - before.bitrate_bps as i128,
        dropped_delta_change: after.dropped_delta as i128 - before.dropped_delta as i128,
        level_change: after.level as i8 - before.level as i8,
    }
}
