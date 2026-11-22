use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "audio-undo-lab", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Action>,
}

#[derive(Debug, Subcommand)]
enum Action {
    Run {
        #[arg(long, default_value = "experiments/audio-undo.undoexp")]
        scenario: PathBuf,
        #[arg(long, default_value = "artifacts/evidence")]
        evidence: PathBuf,
    },
    Replay {
        #[arg(long, default_value = "artifacts/evidence")]
        evidence: PathBuf,
    },
    Ui {
        #[arg(long, default_value = "experiments/audio-undo.undoexp")]
        scenario: PathBuf,
        #[arg(
            long,
            help = "Open the GTK4 window, print a readiness marker, then exit"
        )]
        selftest: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Action::Run {
        scenario: PathBuf::from("experiments/audio-undo.undoexp"),
        evidence: PathBuf::from("artifacts/evidence"),
    }) {
        Action::Run { scenario, evidence } => {
            let report = audio_undo_lab::run_experiment(&scenario, &evidence)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                anyhow::bail!("experiment completed with failures");
            }
        }
        Action::Replay { evidence } => {
            let report = audio_undo_lab::experiment::replay_evidence(&evidence)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if !report.ok {
                anyhow::bail!("replay verification failed");
            }
        }
        #[cfg(feature = "ui")]
        Action::Ui { scenario, selftest } => audio_undo_lab::ui::run(&scenario, selftest)?,
        #[cfg(not(feature = "ui"))]
        Action::Ui { .. } => anyhow::bail!("binary was built without the gtk4 ui feature"),
    }
    Ok(())
}
