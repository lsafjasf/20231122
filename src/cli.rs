use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{analyze, gui};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Rebuild and verify mouse trajectory overlay processing"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Analyze(AnalyzeArgs),
    Verify {
        #[arg(long, default_value = "artifacts/out")]
        out: PathBuf,
    },
    Gui {
        #[arg(long, default_value = "artifacts/out")]
        out: PathBuf,
    },
}

#[derive(Debug, clap::Args)]
struct AnalyzeArgs {
    #[arg(long, default_value = "artifacts/original/screen.mkv")]
    video: PathBuf,
    #[arg(long, default_value = "artifacts/original/mouse-events.jsonl")]
    events: PathBuf,
    #[arg(long, default_value = "artifacts/out")]
    out: PathBuf,
    #[arg(long, default_value_t = 1_000_000)]
    max_events: usize,
    #[arg(long, default_value_t = 134_217_728)]
    max_event_bytes: u64,
    #[arg(long, default_value_t = 0.0)]
    start_ns: f64,
    #[arg(long, default_value_t = 0.0)]
    end_ns: f64,
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Gui {
        out: PathBuf::from("artifacts/out"),
    }) {
        Command::Analyze(args) => {
            gstreamer::init()?;
            let result = analyze::run(analyze::AnalyzeOptions {
                video: args.video,
                events: args.events,
                out: args.out,
                max_events: args.max_events,
                max_event_bytes: args.max_event_bytes,
                start_ns: args.start_ns as u64,
                end_ns: args.end_ns as u64,
            })?;
            println!("status={} anchors={}", result.status, result.anchor_count);
            if result.status == "error" {
                std::process::exit(2);
            }
        }
        Command::Verify { out } => {
            let summary = analyze::verify(&out)?;
            println!("{summary}");
            if !summary.starts_with("PASS") {
                std::process::exit(2);
            }
        }
        Command::Gui { out } => {
            if let Err(error) = gui::run(&out) {
                eprintln!("gui unavailable: {error:#}");
                std::process::exit(2);
            }
        }
    }
    Ok(())
}
