//! Command line entry point for the replayable HLS diagnosis lab.

use hls_resume_lab::cleanup;
use hls_resume_lab::core::journal::Journal;
use hls_resume_lab::core::runner::{self, RunConfig, RunOutcome};
use std::path::Path;
use std::process::ExitCode;

fn usage() -> &'static str {
    "usage:\n  hls-lab run <config.json> [run-id]\n  hls-lab replay <journal.jsonl>\n  hls-lab cleanup <run-id>\n  hls-lab gui   (requires the gtk feature)\n"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let command = args.get(1).map(String::as_str).unwrap_or("help");

    match command {
        "run" => {
            let config_path = match args.get(2) {
                Some(path) => Path::new(path),
                None => {
                    eprint!("{}", usage());
                    return ExitCode::FAILURE;
                }
            };
            let run_id = args
                .get(3)
                .cloned()
                .unwrap_or_else(|| chrono::Local::now().format("run-%Y%m%d-%H%M%S").to_string());
            run_experiment(config_path, &run_id)
        }
        "replay" => match args.get(2) {
            Some(path) => replay_journal(Path::new(path)),
            None => {
                eprint!("{}", usage());
                ExitCode::FAILURE
            }
        },
        "cleanup" => match args.get(2) {
            Some(run_id) => {
                println!(
                    "{}",
                    cleanup::remove_temp_dir(&cleanup::temp_dir_for(run_id))
                );
                ExitCode::SUCCESS
            }
            None => {
                eprint!("{}", usage());
                ExitCode::FAILURE
            }
        },
        "gui" => {
            #[cfg(feature = "gtk")]
            {
                let code = hls_resume_lab::gtk_app::run_app();
                ExitCode::from(code.get())
            }
            #[cfg(not(feature = "gtk"))]
            {
                eprintln!("该二进制未启用 gtk feature，请用 --features gtk 重新构建");
                ExitCode::FAILURE
            }
        }
        _ => {
            print!("{}", usage());
            ExitCode::SUCCESS
        }
    }
}

fn run_experiment(config_path: &Path, run_id: &str) -> ExitCode {
    let config: RunConfig = match std::fs::read_to_string(config_path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(config) => config,
            Err(err) => {
                eprintln!("配置文件解析失败 {config_path:?}: {err}");
                return ExitCode::FAILURE;
            }
        },
        Err(err) => {
            eprintln!("无法读取配置 {config_path:?}: {err}");
            return ExitCode::FAILURE;
        }
    };

    let temp_dir = cleanup::temp_dir_for(run_id);
    let effective = RunConfig {
        temp_dir: temp_dir.clone(),
        ..config
    };

    let outcome: RunOutcome = match runner::run(&effective, run_id) {
        Ok(outcome) => outcome,
        Err(err) => {
            eprintln!("实验运行失败: {err}");
            let _ = std::fs::remove_dir_all(&temp_dir);
            return ExitCode::FAILURE;
        }
    };

    println!("{}", serde_json::to_string_pretty(&outcome.totals).unwrap());
    println!("journal: {}", outcome.journal_path.display());
    println!("report : {}", outcome.report_path.display());

    // End-of-run cleanup: scratch state is removed, evidence is retained.
    println!("{}", cleanup::remove_temp_dir(&temp_dir));
    ExitCode::SUCCESS
}

fn replay_journal(path: &Path) -> ExitCode {
    match Journal::replay(path) {
        Ok(entries) => {
            let total = entries.len();
            for entry in entries {
                println!(
                    "step {:>2} [{:<13}] result={}",
                    entry.step, entry.phase, entry.result
                );
            }
            println!("replayed {total} steps from {}", path.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("回放失败 {path:?}: {err}");
            ExitCode::FAILURE
        }
    }
}
