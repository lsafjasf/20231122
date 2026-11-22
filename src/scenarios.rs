use std::fmt;
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::model::{
    BoundaryEvent, EncodeSample, ProcessedPoint, ProcessingParams, StatisticsProcessor,
    StatsSnapshot, WaveformPoint,
};

pub const DEFAULT_HUGE_SAMPLES: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioKind {
    Empty,
    Boundary,
    Abnormal,
    Huge,
}

impl ScenarioKind {
    pub const ALL: [ScenarioKind; 4] = [
        ScenarioKind::Empty,
        ScenarioKind::Boundary,
        ScenarioKind::Abnormal,
        ScenarioKind::Huge,
    ];
}

impl fmt::Display for ScenarioKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            ScenarioKind::Empty => "empty",
            ScenarioKind::Boundary => "boundary",
            ScenarioKind::Abnormal => "abnormal",
            ScenarioKind::Huge => "huge",
        };
        formatter.write_str(value)
    }
}

impl FromStr for ScenarioKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "empty" => Ok(ScenarioKind::Empty),
            "boundary" => Ok(ScenarioKind::Boundary),
            "abnormal" => Ok(ScenarioKind::Abnormal),
            "huge" => Ok(ScenarioKind::Huge),
            _ => Err(format!(
                "unknown scenario {value:?}; expected empty|boundary|abnormal|huge"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioRun {
    pub scenario: ScenarioKind,
    pub parameters: ProcessingParams,
    pub parameter_changes: Vec<String>,
    pub snapshot: StatsSnapshot,
    pub processed_points: Vec<ProcessedPoint>,
    pub boundaries: Vec<BoundaryEvent>,
    pub waveform: Vec<WaveformPoint>,
    pub first_input: Option<EncodeSample>,
    pub last_input: Option<EncodeSample>,
    pub retained_inputs: Vec<EncodeSample>,
    pub generated_samples: u64,
    pub rejected_samples: u64,
    pub requested_huge_samples: Option<u64>,
    pub elapsed: Duration,
}

pub fn run_scenario(kind: ScenarioKind, huge_samples: u64) -> ScenarioRun {
    let requested = match kind {
        ScenarioKind::Huge => Some(huge_samples.max(1)),
        _ => None,
    };
    let (parameters, parameter_changes) = parameters_for(kind).sanitized();
    let mut processor = StatisticsProcessor::new(parameters.clone());
    let started = Instant::now();
    let mut generated_samples = 0_u64;
    let mut first_input = None;
    let mut last_input = None;
    let mut retained_inputs = Vec::new();

    let mut ingest = |sample: EncodeSample,
                      processor: &mut StatisticsProcessor,
                      retained_inputs: &mut Vec<EncodeSample>| {
        if first_input.is_none() {
            first_input = Some(sample.clone());
        }
        last_input = Some(sample.clone());
        if retained_inputs.len() < 256 {
            retained_inputs.push(sample.clone());
        }
        generated_samples = generated_samples.saturating_add(1);
        let _ = processor.ingest(sample);
    };

    match kind {
        ScenarioKind::Empty => {}
        ScenarioKind::Boundary => {
            for sample in boundary_samples(&parameters) {
                ingest(sample, &mut processor, &mut retained_inputs);
            }
        }
        ScenarioKind::Abnormal => {
            for sample in abnormal_samples(&parameters) {
                ingest(sample, &mut processor, &mut retained_inputs);
            }
        }
        ScenarioKind::Huge => {
            for index in 0..huge_samples.max(1) {
                let sample = huge_sample(index);
                ingest(sample, &mut processor, &mut retained_inputs);
            }
        }
    }

    let snapshot = processor.snapshot();
    let rejected_samples = snapshot.rejected_samples;
    let processed_points = processor.points();
    ScenarioRun {
        scenario: kind,
        parameters,
        parameter_changes,
        waveform: snapshot.waveform.clone(),
        snapshot,
        processed_points,
        boundaries: processor.boundaries().to_vec(),
        first_input,
        last_input,
        retained_inputs,
        generated_samples,
        rejected_samples,
        requested_huge_samples: requested,
        elapsed: started.elapsed(),
    }
}

fn parameters_for(kind: ScenarioKind) -> ProcessingParams {
    match kind {
        ScenarioKind::Abnormal | ScenarioKind::Huge => ProcessingParams::extreme(),
        ScenarioKind::Empty | ScenarioKind::Boundary => ProcessingParams::default(),
    }
}

fn boundary_samples(params: &ProcessingParams) -> Vec<EncodeSample> {
    const STEP_NS: u64 = 33_333_333;
    let base = 1_000_000_000_u64;
    let warn = params.fps_warn;
    let critical = params.fps_critical;
    let bitrate_warn = params.bitrate_warn_bps;
    let bitrate_critical = params.bitrate_critical_bps;

    let values = [
        (base, 100, 30.0, 4_000_000, 0_u64),
        (base + STEP_NS, 101, warn + 0.0001, 4_000_000, 0),
        (base + 2 * STEP_NS, 102, warn - 0.0001, 4_000_000, 0),
        (base + 3 * STEP_NS, 103, critical + 0.0001, 4_000_000, 0),
        (base + 4 * STEP_NS, 104, critical - 0.0001, 4_000_000, 0),
        (base + 5 * STEP_NS, 105, 30.0, bitrate_warn - 1, 0),
        (base + 6 * STEP_NS, 106, 30.0, bitrate_warn + 1, 0),
        (base + 7 * STEP_NS, 107, 30.0, bitrate_critical + 1, 0),
        (base + 8 * STEP_NS, 108, 30.0, 4_000_000, 1),
        (base + 9 * STEP_NS, 109, 30.0, 4_000_000, 6),
        (base + 10 * STEP_NS, 110, 30.0, 4_000_000, 6),
    ];

    let samples: Vec<_> = values
        .into_iter()
        .enumerate()
        .map(|(index, (timestamp, frame, fps, bitrate, dropped))| {
            EncodeSample::new(
                timestamp,
                7,
                frame,
                fps,
                bitrate,
                dropped,
                (index as u64 + 1) * 2_000,
            )
        })
        .collect();

    samples
}

fn abnormal_samples(params: &ProcessingParams) -> Vec<EncodeSample> {
    const STEP_NS: u64 = 16_666_667;
    let base = u64::MAX - 2_000_000_000;
    let fps_values = [
        240.0,
        params.fps_warn - 0.001,
        params.fps_critical + 0.001,
        0.0,
        f64::NAN,
        -10.0,
        f64::INFINITY,
        f64::MAX,
        239.999,
    ];
    let bitrate_values = [
        1_000_000_u64,
        params.bitrate_warn_bps + 1,
        params.bitrate_critical_bps + 1,
        10_000_000_000,
        u64::MAX,
        2_000_000,
    ];
    let dropped_values = [0_u64, 1, 2, 17, u64::MAX, 0, 9];
    let mut samples = Vec::with_capacity(144);

    for index in 0..144_u64 {
        let track = (index % 8) as u32;
        let frame = if index == 71 {
            10
        } else if index == 72 {
            9
        } else {
            index + 1
        };
        let timestamp = if index == 96 {
            base - 60_000_000_000
        } else {
            base.saturating_add(index * STEP_NS)
        };
        samples.push(EncodeSample::new(
            timestamp,
            track,
            frame,
            fps_values[index as usize % fps_values.len()],
            bitrate_values[index as usize % bitrate_values.len()],
            dropped_values[index as usize % dropped_values.len()],
            if index == 97 {
                0
            } else {
                index.saturating_mul(8_192)
            },
        ));
    }
    samples
}

fn huge_sample(index: u64) -> EncodeSample {
    const STEP_NS: u64 = 33_333_333;
    let track = (index % 8) as u32;
    let frame = index / 8;
    let fps = match index % 997 {
        0 if index > 0 => 0.5,
        1 => 240.0,
        2 => 120.0,
        _ => 30.0 + (index % 17) as f64 * 0.01,
    };
    let bitrate = match index % 1_499 {
        0 if index > 0 => u64::MAX,
        1 => 9_999_999,
        _ => 2_000_000 + (index % 200) * 10_000,
    };
    let dropped = (index / 97).saturating_mul(3);
    EncodeSample::new(
        index.saturating_mul(STEP_NS),
        track,
        frame,
        fps,
        bitrate,
        dropped,
        index.saturating_mul(1_024),
    )
}
