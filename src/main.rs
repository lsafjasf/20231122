use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use mask_edge_review::media::{ensure_gstreamer, probe_media};
use mask_edge_review::project::{create_project, inspect_project, open_project};
use mask_edge_review::service::{acceptance_value, begin_run, execute_run, finish_run};
use mask_edge_review::{ui, APP_NAME};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("错误：{error:#}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        ensure_gstreamer()?;
        ui::launch(None)?;
        return Ok(ExitCode::SUCCESS);
    }
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(ExitCode::SUCCESS);
    }
    if args.iter().any(|arg| arg == "--version" || arg == "-V") {
        println!("{APP_NAME} {}", env!("CARGO_PKG_VERSION"));
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(source) = flag_value(&args, "--import") {
        let root = flag_value(&args, "--project-dir")
            .or_else(|| flag_value(&args, "--project"))
            .ok_or_else(|| anyhow::anyhow!("--import 需要 --project-dir"))?;
        let title = flag_value(&args, "--title").unwrap_or_else(|| {
            PathBuf::from(&source)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("review")
                .to_string()
        });
        ensure_gstreamer()?;
        let media = probe_media(PathBuf::from(&source).as_path())?;
        let opened = create_project(
            PathBuf::from(root).as_path(),
            PathBuf::from(&source).as_path(),
            title,
            media,
        )?;
        println!(
            "{}",
            serde_json::to_string_pretty(&acceptance_value(&opened.session))?
        );
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(root) = flag_value(&args, "--inspect") {
        let opened = inspect_project(PathBuf::from(root).as_path())?;
        let report = acceptance_value(&opened.session);
        println!("{}", serde_json::to_string_pretty(&report)?);
        let verified = opened.integrity.verified;
        return Ok(if verified {
            ExitCode::SUCCESS
        } else {
            ExitCode::from(2)
        });
    }
    if let Some(root) = flag_value(&args, "--render") {
        let segment = flag_value(&args, "--segment")
            .ok_or_else(|| anyhow::anyhow!("--render 需要 --segment 1..3"))?
            .parse::<usize>()
            .context("--segment 必须是 1、2 或 3")?;
        if !(1..=3).contains(&segment) {
            bail!("--segment 必须是 1、2 或 3");
        }
        ensure_gstreamer()?;
        let mut opened = open_project(PathBuf::from(&root).as_path())?;
        if !opened.integrity.verified {
            bail!("原始素材校验失败：{}", opened.integrity.message);
        }
        let index = segment - 1;
        let job = begin_run(&mut opened.session, index)?;
        let result = execute_run(&job);
        let outcome = match result {
            Ok(outcome) => {
                finish_run(&mut opened.session, index, &job.run_id, Ok(outcome.clone()))?;
                outcome
            }
            Err(error) => {
                finish_run(
                    &mut opened.session,
                    index,
                    &job.run_id,
                    Err(format!("{error:#}")),
                )?;
                return Err(error);
            }
        };
        if let Some(destination) = flag_value(&args, "--output") {
            let destination = PathBuf::from(destination);
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&outcome.output_path, &destination)
                .with_context(|| format!("无法复制导出结果到 {}", destination.display()))?;
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "segment": segment,
                "run_number": job.run_number,
                "output": outcome.output_path,
                "output_sha256": outcome.output_sha256,
                "measured_start_ns": outcome.measured_start_ns,
                "measured_end_ns": outcome.measured_end_ns,
                "evidence": outcome.evidence,
            }))?
        );
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(project) = flag_value(&args, "--project") {
        ensure_gstreamer()?;
        ui::launch(Some(PathBuf::from(project)))?;
        return Ok(ExitCode::SUCCESS);
    }
    print_help();
    Ok(ExitCode::from(1))
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn print_help() {
    println!(
        "{APP_NAME} {}\n\
         \n\
         用法：\n\
         \x20 mask-edge-review [--project 项目目录]\n\
         \x20 mask-edge-review --import 视频 --project-dir 新项目目录 [--title 标题]\n\
         \x20 mask-edge-review --inspect 项目目录\n\
         \x20 mask-edge-review --render 项目目录 --segment 1|2|3 [--output 文件.webm]\n\
         \n\
         无参数启动 GTK4 工作台，并自动恢复最近项目。\n\
         --inspect 输出锚点、原始偏移、绝对时间、完整性及证据校验 JSON。",
        env!("CARGO_PKG_VERSION")
    );
}
