use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use tempo_pitch_debug::audit::{canonical_record_bytes, load_record, verify_artifact};
use tempo_pitch_debug::engine::{run_with_renderer, RunRequest};
use tempo_pitch_debug::gst_engine::GStreamerRenderer;
use tempo_pitch_debug::model::{
    Limits, TimeSpan, DEFAULT_PITCH_GAP_US, DEFAULT_RENDER_QUANTUM_US, DEFAULT_WINDOW_US,
};

#[derive(Debug, Parser)]
#[command(
    name = "tempo-pitch-debug",
    about = "Auditable tempo/pitch debug playback (Rust + GStreamer)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run a new debug render from explicit inputs.
    Render(RenderArgs),
    /// Run using a scenario JSON file.
    Scenario(ScenarioArgs),
    /// Reload an artifact, verify hashes and recompute the waveform.
    Verify {
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Reload record.json and prove field/order-stable canonical serialization.
    Reload {
        #[arg(long)]
        artifact: PathBuf,
        #[arg(long)]
        print: bool,
    },
}

#[derive(Debug, Args)]
struct RenderArgs {
    #[arg(long)]
    audio: PathBuf,
    #[arg(long)]
    speed: PathBuf,
    #[arg(long)]
    pitch: PathBuf,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value = "manual-run")]
    name: String,
    #[arg(long, default_value_t = 0)]
    channel: u16,
    #[arg(long, default_value_t = DEFAULT_WINDOW_US)]
    window_us: u64,
    #[arg(long, default_value_t = DEFAULT_RENDER_QUANTUM_US)]
    render_quantum_us: u64,
    #[arg(long, default_value_t = DEFAULT_PITCH_GAP_US)]
    pitch_gap_us: u64,
    #[arg(long, num_args = 2, value_names = ["START_US", "END_US"])]
    missing_span: Option<Vec<u64>>,
    #[arg(long, default_value_t = Limits::default().max_audio_bytes)]
    max_audio_bytes: u64,
    #[arg(long, default_value_t = Limits::default().max_curve_bytes)]
    max_curve_bytes: u64,
    #[arg(long, default_value_t = Limits::default().max_curve_points)]
    max_curve_points: u64,
    #[arg(long, default_value_t = Limits::default().max_timeline_entries)]
    max_timeline_entries: u64,
    #[arg(long, default_value_t = Limits::default().max_render_segments)]
    max_render_segments: u64,
    #[arg(long, default_value_t = Limits::default().max_decoded_pcm_bytes)]
    max_decoded_pcm_bytes: u64,
}

#[derive(Debug, Args)]
struct ScenarioArgs {
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    out: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match execute(cli.command) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn execute(command: Command) -> anyhow::Result<ExitCode> {
    match command {
        Command::Render(args) => {
            let missing_span = match args.missing_span {
                Some(values) if values.len() == 2 => Some(TimeSpan {
                    start_us: values[0],
                    end_us: values[1],
                }),
                Some(_) => anyhow::bail!("--missing-span requires START_US END_US"),
                None => None,
            };
            let request = RunRequest {
                audio_path: args.audio,
                speed_curve_path: args.speed,
                pitch_track_path: args.pitch,
                output_dir: args.out,
                scenario_name: args.name,
                analysed_channel: args.channel,
                window_us: args.window_us,
                render_quantum_us: args.render_quantum_us,
                pitch_gap_threshold_us: args.pitch_gap_us,
                missing_span,
                limits: Limits {
                    max_audio_bytes: args.max_audio_bytes,
                    max_curve_bytes: args.max_curve_bytes,
                    max_curve_points: args.max_curve_points,
                    max_timeline_entries: args.max_timeline_entries,
                    max_render_segments: args.max_render_segments,
                    max_decoded_pcm_bytes: args.max_decoded_pcm_bytes,
                },
            };
            run(request)
        }
        Command::Scenario(args) => {
            let request = RunRequest::from_scenario(&args.config, args.out)?;
            run(request)
        }
        Command::Verify { artifact, json } => {
            let report = verify_artifact(&artifact)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report.checks)?);
            } else {
                for check in &report.checks {
                    println!(
                        "{} {}: {}",
                        if check.passed { "PASS" } else { "FAIL" },
                        check.name,
                        check.detail
                    );
                }
            }
            if report.passed() {
                println!("artifact verified: {}", artifact.display());
                Ok(ExitCode::SUCCESS)
            } else {
                eprintln!("artifact verification failed: {}", artifact.display());
                Ok(ExitCode::from(1))
            }
        }
        Command::Reload { artifact, print } => {
            let path = artifact.join("record.json");
            let record = load_record(&path)?;
            let canonical = canonical_record_bytes(&record)?;
            let stored = std::fs::read(&path)?;
            let stable = canonical == stored;
            println!(
                "reload {}: status={:?}, timeline_rows={}, fields={}",
                if stable { "OK" } else { "MISMATCH" },
                record.status,
                record.timeline.len(),
                record.recording_policy.stable_field_order.join(",")
            );
            println!(
                "anomalies={}, output_frames={}, output_sha256={}",
                record.anomalies.len(),
                record.output.frames,
                record.output.sha256
            );
            if print {
                print!("{}", String::from_utf8(canonical)?);
            }
            if stable {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
    }
}

fn run(request: RunRequest) -> anyhow::Result<ExitCode> {
    let mut renderer = GStreamerRenderer::new()?;
    let record = run_with_renderer(request, &mut renderer)?;
    let artifact = PathBuf::from(&record.request.output_dir);
    println!(
        "status={:?} job={} timeline_rows={} anomalies={}",
        record.status,
        record.job_id,
        record.timeline.len(),
        record.anomalies.len()
    );
    println!(
        "output={} frames={} sha256={}",
        artifact.join("output.wav").display(),
        record.output.frames,
        record.output.sha256
    );
    println!(
        "verify: tempo-pitch-debug verify --artifact {}",
        artifact.display()
    );
    Ok(ExitCode::SUCCESS)
}
