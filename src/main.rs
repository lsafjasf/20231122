use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Result, bail};
use encode_panel::evidence::{CaptureEvidence, SCHEMA_VERSION, toolchain_evidence};
use encode_panel::model::{ProcessingParams, StatisticsProcessor};
use encode_panel::pipeline::{LiveCapture, PipelineConfig};
use encode_panel::scenarios::DEFAULT_HUGE_SAMPLES;
use encode_panel::{
    EvidenceBundle, ScenarioKind, run_scenario, write_capture_evidence, write_evidence,
};

#[derive(Debug)]
struct Args {
    headless: bool,
    scenario: Option<ScenarioKind>,
    huge_samples: u64,
    output: PathBuf,
    gstreamer: bool,
    duration_ms: u64,
    extreme_pipeline: bool,
    ui_smoke_ms: Option<u64>,
    screenshot: Option<PathBuf>,
    help: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            headless: false,
            scenario: None,
            huge_samples: DEFAULT_HUGE_SAMPLES,
            output: PathBuf::from("artifacts/evidence"),
            gstreamer: false,
            duration_ms: 3_000,
            extreme_pipeline: false,
            ui_smoke_ms: None,
            screenshot: None,
            help: false,
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.help {
        print_help();
        return Ok(());
    }

    if args.headless {
        run_headless(&args)?;
        return Ok(());
    }

    let config = pipeline_config(&args);
    encode_panel::ui::run(config, args.ui_smoke_ms, args.screenshot)
}

fn run_headless(args: &Args) -> Result<()> {
    let scenarios: Vec<_> = match args.scenario {
        Some(ScenarioKind::Huge) if args.scenario.is_some() => vec![ScenarioKind::Huge],
        Some(kind) => vec![kind],
        None => ScenarioKind::ALL.to_vec(),
    };

    if !args.gstreamer || args.scenario.is_some() {
        for scenario in scenarios {
            let run = run_scenario(scenario, args.huge_samples);
            let bundle = EvidenceBundle::from_scenario(&run);
            let paths = write_evidence(&args.output, &bundle)?;
            println!(
                "{:<9} samples={:<8} rejected={:<3} boundaries={:<5} waveform={:<4} elapsed={:>8.3}ms",
                scenario.to_string(),
                run.generated_samples,
                run.rejected_samples,
                run.snapshot.boundary_events_total,
                run.snapshot.waveform.len(),
                run.elapsed.as_secs_f64() * 1_000.0,
            );
            println!("          evidence={}", paths.evidence.display());
            println!("          waveform={}", paths.waveform.display());
        }
    }

    if args.gstreamer {
        run_gstreamer_capture(args)?;
    }
    Ok(())
}

fn run_gstreamer_capture(args: &Args) -> Result<()> {
    let config = pipeline_config(args);
    let params = if args.extreme_pipeline {
        ProcessingParams::extreme()
    } else {
        ProcessingParams::default()
    };
    let mut processor = StatisticsProcessor::new(params.clone());
    let mut capture = LiveCapture::start(config)?;
    let mut first_sample = None;
    let mut last_sample = None;
    let mut ingest_errors = 0_u64;
    let report = capture.capture_into(Duration::from_millis(args.duration_ms), |sample| {
        if first_sample.is_none() {
            first_sample = Some(sample.clone());
        }
        last_sample = Some(sample.clone());
        if processor.ingest(sample).is_err() {
            ingest_errors = ingest_errors.saturating_add(1);
        }
    });
    let snapshot = processor.snapshot();
    let evidence = CaptureEvidence {
        schema_version: SCHEMA_VERSION.to_string(),
        created_at_unix_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        toolchain: toolchain_evidence(),
        report: report.clone(),
        parameters: params,
        summary: snapshot,
        boundaries: processor.boundaries().to_vec(),
        waveform_points: processor.snapshot().waveform,
        first_sample,
        last_sample,
        ingest_errors,
    };
    let paths = write_capture_evidence(&args.output, &evidence)?;
    println!(
        "gstreamer samples={} expected~={} dropped={} errors={} eos={}",
        report.samples_delivered,
        report.expected_frames,
        report.dropped_frames_detected,
        report.bus_errors.len(),
        report.eos,
    );
    println!("          pipeline={}", report.pipeline_description);
    println!("          evidence={}", paths.evidence.display());
    println!("          waveform={}", paths.waveform.display());
    Ok(())
}

fn pipeline_config(args: &Args) -> PipelineConfig {
    let mut config = if args.extreme_pipeline {
        PipelineConfig::extreme()
    } else {
        PipelineConfig::default()
    };
    config.duration_ms = args.duration_ms;
    config
}

impl Args {
    fn parse(iter: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = Args::default();
        let mut values = iter.peekable();
        while let Some(argument) = values.next() {
            match argument.as_str() {
                "--headless" => args.headless = true,
                "--gstreamer" => args.gstreamer = true,
                "--extreme-pipeline" => args.extreme_pipeline = true,
                "--scenario" => {
                    let value = values
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--scenario requires a value"))?;
                    args.scenario = Some(
                        ScenarioKind::from_str(&value).map_err(|error| anyhow::anyhow!(error))?,
                    );
                }
                "--huge-samples" => {
                    let value = values
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--huge-samples requires a value"))?;
                    args.huge_samples = parse_count(&value)?;
                }
                "--duration-ms" => {
                    let value = values
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--duration-ms requires a value"))?;
                    args.duration_ms = value.parse()?;
                }
                "--out" => {
                    args.output = PathBuf::from(
                        values
                            .next()
                            .ok_or_else(|| anyhow::anyhow!("--out requires a value"))?,
                    );
                }
                "--ui-smoke-ms" => {
                    let value = values
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("--ui-smoke-ms requires a value"))?;
                    args.ui_smoke_ms = Some(value.parse()?);
                }
                "--screenshot" => {
                    args.screenshot =
                        Some(PathBuf::from(values.next().ok_or_else(|| {
                            anyhow::anyhow!("--screenshot requires a value")
                        })?));
                }
                "--help" | "-h" => args.help = true,
                other => bail!("unknown argument {other:?}; use --help"),
            }
        }
        if args.scenario.is_some() && !args.headless {
            bail!("--scenario requires --headless");
        }
        if args.huge_samples == 0 {
            bail!("--huge-samples must be greater than zero");
        }
        if args.screenshot.is_some() && args.ui_smoke_ms.is_none() {
            bail!("--screenshot requires --ui-smoke-ms");
        }
        Ok(args)
    }
}

fn parse_count(value: &str) -> Result<u64> {
    let normalized = value.trim().to_ascii_lowercase();
    let (number, multiplier) = match normalized.strip_suffix('m') {
        Some(number) => (number, 1_000_000_u64),
        None => match normalized.strip_suffix('k') {
            Some(number) => (number, 1_000_u64),
            None => (normalized.as_str(), 1_u64),
        },
    };
    let count: u64 = number.parse()?;
    Ok(count.saturating_mul(multiplier))
}

fn print_help() {
    println!(
        "\
Encode Pulse - realtime encoding statistics panel (Rust + GStreamer + GTK4)

USAGE:
  encode-panel [OPTIONS]
  encode-panel --headless [--scenario <kind>] [--gstreamer]

OPTIONS:
  --headless            Generate deterministic evidence instead of opening GTK4.
  --scenario <kind>     empty | boundary | abnormal | huge (default: all).
  --huge-samples <n>    Huge-input count, supports 1m/500k suffixes (default: 1m).
  --out <dir>           Evidence root (default: artifacts/evidence).
  --gstreamer           Run the real videotestsrc/x264/appsink capture.
  --duration-ms <n>     Live capture duration (default: 3000).
  --extreme-pipeline    Use 1920x1080@240, 32 kbps, 25% drop, 1-buffer sink.
  --ui-smoke-ms <n>     Auto-close the GTK4 window after n milliseconds.
  --screenshot <path>   Save the GTK4 window as PNG when --ui-smoke-ms fires.
  -h, --help            Show this help.
"
    );
}
