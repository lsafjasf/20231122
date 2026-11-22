use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use media_meta_replay::experiment::Experiment;
use media_meta_replay::fixture::{demo_record, truncated_fixture_bytes, write_demo_media};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        None | Some("replay") => replay_command(args.next()),
        Some("init-fixtures") => {
            init_fixtures(args.next().as_deref().unwrap_or("fixtures"))?;
            Ok(())
        }
        Some("summary") => {
            summary_command(args.next().as_deref().unwrap_or("artifacts/experiment"))
        }
        Some("probe") => {
            let path = args
                .next()
                .ok_or("usage: media-meta-replay probe <media-path>")?;
            let output = args.next();
            probe_command(Path::new(&path), output.as_deref().map(Path::new))
        }
        Some("help" | "--help" | "-h") => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown command: {command}").into()),
    }
}

fn print_help() {
    println!(
        "媒体元数据对照实验\n\n\
         用法：\n\
           media-meta-replay replay [输出目录]\n\
           media-meta-replay init-fixtures [目录]\n\
           media-meta-replay summary [实验目录]\n\
           media-meta-replay probe <媒体路径> [输出 JSON]   # 需要 --features desktop\n\n\
         默认 replay 输出到 artifacts/experiment。"
    );
}

fn replay_command(output: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(output.unwrap_or_else(|| "artifacts/experiment".to_owned()));
    let experiment = Experiment::builtin()?;
    let bundle = experiment.write_artifacts(&output)?;
    let summary = &bundle.summary;
    println!("实验记录：{}", summary.record_id);
    println!(
        "恢复控制：{} 步，最终状态 {:?}，全部接受={}",
        summary.control.steps_executed, summary.control.final_phase, summary.control.all_accepted
    );
    println!(
        "截断记录：完整 {} 行 / {} 个输入，缺失尾部 {} bytes",
        summary.truncation.complete_lines,
        summary.truncation.complete_inputs,
        summary.truncation.incomplete_tail_bytes
    );
    for case in &summary.cases {
        if let Some(error) = &case.first_error {
            println!(
                "{}：首错 #{} {} / {}；最后有效 phase={:?} c{}，首错状态={:?} c{}",
                case.name,
                error.step_index,
                error.step_id,
                error.rule,
                error.state_before.phase,
                error.state_before.cursor,
                error.state_after.phase,
                error.state_after.cursor
            );
        } else if let Some(divergence) = &case.comparison_to_control.first_divergence {
            println!(
                "{}：无规则错误；首个分歧 #{} {}",
                case.name, divergence.index, divergence.reason
            );
        } else {
            println!("{}：无规则错误且与恢复对照一致", case.name);
        }
    }
    if let Some(error) = &summary.first_error_across_scenarios {
        println!(
            "跨场景最早错误：{} #{} {}",
            error.scenario, error.step_index, error.rule
        );
    }
    println!("证据目录：{}", bundle.root.display());
    Ok(())
}

fn init_fixtures(root: &str) -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(root);
    fs::create_dir_all(root.join("media"))?;
    let record = demo_record();
    let mut recovered = serde_json::to_vec_pretty(&record)?;
    recovered.push(b'\n');
    fs::write(root.join("recovered-record.json"), recovered)?;
    fs::write(
        root.join("truncated-record.jsonl"),
        truncated_fixture_bytes(&record, 5)?,
    )?;
    write_demo_media(&root.join("media/take-001.wav"))?;
    println!("fixtures written to {}", root.display());
    Ok(())
}

fn summary_command(root: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(root).join("summary.json");
    let bytes = fs::read(path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

#[cfg(feature = "desktop")]
fn probe_command(path: &Path, output: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    let probe = media_meta_replay::media::probe_media(path)?;
    let json = serde_json::to_string_pretty(&probe)?;
    if let Some(output) = output {
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(output, format!("{json}\n"))?;
        println!("probe evidence written to {}", output.display());
    } else {
        println!("{json}");
    }
    Ok(())
}

#[cfg(not(feature = "desktop"))]
fn probe_command(_path: &Path, _output: Option<&Path>) -> Result<(), Box<dyn std::error::Error>> {
    Err("probe requires: cargo run --features desktop -- probe <media-path>".into())
}
