use crate::engine::{self, ProcessOptions, RunPaths};
use crate::model::{CalibrationConfig, RunState};
use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use serde_json::json;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(
    name = "loudness-calibrate",
    version,
    about = "三段锚点响度校准：可暂停、可恢复、可复核"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 新建运行并在三段锚点间暂停/完成
    Run {
        #[arg(long, default_value = "assets/original/loudness_original.wav")]
        source: PathBuf,
        #[arg(long, default_value = "artifacts/runs/demo")]
        run: PathBuf,
        #[arg(long, default_value_t = -20.0)]
        target_dbfs: f64,
        #[arg(long, value_name = "A|B|C|1|2|3")]
        pause_after: Option<String>,
        #[arg(long)]
        force_degrade: bool,
        #[arg(long)]
        restart: bool,
    },
    /// 从已保存锚点恢复
    Resume {
        #[arg(long, default_value = "artifacts/runs/demo")]
        run: PathBuf,
        #[arg(long, value_name = "A|B|C|1|2|3")]
        pause_after: Option<String>,
    },
    /// 打印断点状态
    Status {
        #[arg(long, default_value = "artifacts/runs/demo")]
        run: PathBuf,
    },
    /// 重新计算证据并输出验收行
    Verify {
        #[arg(long, default_value = "artifacts/runs/demo")]
        run: PathBuf,
    },
    /// 比较连续运行与暂停恢复运行
    Compare {
        #[arg(long)]
        a: PathBuf,
        #[arg(long)]
        b: PathBuf,
        #[arg(long, default_value = "artifacts/compare/equivalence.json")]
        out: PathBuf,
    },
    /// 打开 GTK4 工作台
    Gui {
        #[arg(long, default_value = "assets/original/loudness_original.wav")]
        source: PathBuf,
        #[arg(long, default_value = "artifacts/runs/gui")]
        run: PathBuf,
    },
}

pub fn main_entry() -> ExitCode {
    match execute() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("错误: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn execute() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Gui {
        source: PathBuf::from("assets/original/loudness_original.wav"),
        run: PathBuf::from("artifacts/runs/gui"),
    }) {
        Command::Run {
            source,
            run,
            target_dbfs,
            pause_after,
            force_degrade,
            restart,
        } => {
            if run.join("state.json").exists() {
                if !restart {
                    bail!(
                        "运行目录已有断点: {}。使用 --restart 重新开始，或执行 resume。",
                        run.display()
                    );
                }
                std::fs::remove_dir_all(&run)?;
            }
            let config = CalibrationConfig {
                target_rms_dbfs: target_dbfs,
                force_degrade,
                ..CalibrationConfig::default()
            };
            let pause_index = parse_pause(pause_after.as_deref())?;
            let (paths, state) = engine::init_run(&source, &run, config)?;
            print_started(&state);
            let outcome = engine::process(
                &paths,
                state,
                ProcessOptions {
                    pause_after: pause_index,
                    ..ProcessOptions::default()
                },
            )?;
            print_outcome(&outcome.state, outcome.paused);
            Ok(ExitCode::SUCCESS)
        }
        Command::Resume { run, pause_after } => {
            let paths = RunPaths::new(&run);
            let pause_index = parse_pause(pause_after.as_deref())?;
            let outcome = engine::resume(
                &paths,
                ProcessOptions {
                    pause_after: pause_index,
                    ..ProcessOptions::default()
                },
            )?;
            print_outcome(&outcome.state, outcome.paused);
            Ok(ExitCode::SUCCESS)
        }
        Command::Status { run } => {
            let paths = RunPaths::new(&run);
            let state = engine::load_state(&paths)?;
            print_status(&state)?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Verify { run } => {
            let paths = RunPaths::new(&run);
            let report = crate::evidence::verify_run(&paths)?;
            for check in &report.checks {
                println!(
                    "{} {:<36} {}",
                    if check.ok { "PASS" } else { "FAIL" },
                    check.name,
                    check.detail
                );
            }
            println!("--- acceptance ---");
            for line in &report.acceptance_lines {
                println!("{line}");
            }
            Ok(if report.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Compare { a, b, out } => {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let report =
                crate::evidence::compare_runs(&RunPaths::new(&a), &RunPaths::new(&b), &out)?;
            println!(
                "{} continuous={} paused={} outputs_identical={} report={}",
                if report.ok { "PASS" } else { "FAIL" },
                report.a_output_sha256.as_deref().unwrap_or("none"),
                report.b_output_sha256.as_deref().unwrap_or("none"),
                report.outputs_identical,
                out.display()
            );
            for anchor in &report.anchors {
                println!(
                    "{} before={} after={} gain={} segment_sha={}",
                    anchor.anchor_id,
                    anchor.same_before_rms,
                    anchor.same_after_rms,
                    anchor.same_gain,
                    anchor.same_output_sha256
                );
            }
            Ok(if report.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Command::Gui { source, run } => crate::gui::run(source, run),
    }
}

pub fn parse_pause(value: Option<&str>) -> Result<Option<usize>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let index = match value.trim().to_ascii_uppercase().as_str() {
        "A" | "1" => 0,
        "B" | "2" => 1,
        "C" | "3" => 2,
        other => bail!("无效锚点 {other}，应为 A/B/C 或 1/2/3"),
    };
    Ok(Some(index))
}

fn print_started(state: &RunState) {
    println!(
        "run={} source={} sha256={} duration_ns={} mode={}",
        state.run_id,
        state.source.path,
        state.source.sha256,
        state.source.duration_ns,
        state.resource.mode
    );
    if let Some(degradation) = &state.degradation {
        println!(
            "DEGRADED rate={}Hz reasons={}",
            degradation.output_sample_rate,
            degradation.reasons.join("|")
        );
    }
}

fn print_outcome(state: &RunState, paused: bool) {
    for checkpoint in &state.completed {
        println!(
            "{} t={:.9}s frame={} file_offset={} before={:+.3} after={:+.3} gain={:+.3} saved={}",
            checkpoint.anchor_id,
            checkpoint.anchor_ns as f64 / 1e9,
            checkpoint.anchor_frame,
            checkpoint.original_anchor_file_byte,
            checkpoint.before.rms_dbfs,
            checkpoint.after.rms_dbfs,
            checkpoint.gain.applied_gain_db,
            checkpoint.checkpoint_sha256
        );
    }
    println!(
        "status={} paused={} next_segment={} resumes={}",
        state.status, paused, state.next_segment, state.resume_count
    );
    if let Some(final_output) = &state.final_output {
        println!(
            "FINAL {} frames={} duration_ns={} sha256={}",
            final_output.path,
            final_output.frame_count,
            final_output.duration_ns,
            final_output.sha256
        );
    }
}

fn print_status(state: &RunState) -> Result<()> {
    let value = json!({
        "run_id": state.run_id,
        "status": state.status,
        "mode": state.resource.mode,
        "degradation": state.degradation,
        "source_sha256": state.source.sha256,
        "duration_ns": state.source.duration_ns,
        "next_segment": state.next_segment,
        "resume_count": state.resume_count,
        "anchors": state.completed.iter().map(|item| json!({
            "id": item.anchor_id,
            "time_ns": item.anchor_ns,
            "frame": item.anchor_frame,
            "original_offset": item.original_anchor_file_byte,
            "before_rms_dbfs": item.before.rms_dbfs,
            "after_rms_dbfs": item.after.rms_dbfs,
            "gain_db": item.gain.applied_gain_db,
            "output_sha256": item.output_sha256,
            "checkpoint_sha256": item.checkpoint_sha256
        })).collect::<Vec<_>>(),
        "final_output": state.final_output,
        "resource": state.resource,
    });
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
