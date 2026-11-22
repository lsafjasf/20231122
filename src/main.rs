use proxyrecover::{experiment, replay, report};
use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> &'static str {
    "proxyrecover - 素材代理失效恢复可回放实验\n\
     \n\
     usage:\n\
     \x20 proxyrecover run [--out DIR]                 运行实验，生成 journal/evidence/REPORT.md\n\
     \x20 proxyrecover replay <JOURNAL>                从日志重建并逐条比较\n\
     \x20 proxyrecover verify-artifacts <MANIFEST>     独立校验证据 SHA-256 清单\n\
     \x20 proxyrecover gui                            启动 GTK4 界面\n\
     \x20 proxyrecover gst-env [DIR]                  仅记录 GStreamer 环境"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("") {
        "run" => run_command(&args),
        "replay" => replay_command(&args),
        "verify-artifacts" => verify_command(&args),
        "gui" => {
            proxyrecover::gui::run();
            ExitCode::SUCCESS
        }
        "gst-env" => {
            let dir = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("artifacts"));
            match proxyrecover::gst::record_gst_env(&dir.join("gst-env.txt")) {
                Ok(path) => {
                    println!("wrote {}", path.display());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("gst-env failed: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        "help" | "--help" | "-h" => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn run_command(args: &[String]) -> ExitCode {
    let out = flag_value(args, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("artifacts/latest"));
    let mut progress = |message: &str| eprintln!("[step] {message}");
    match experiment::run_experiment(&out, &mut progress) {
        Ok(output) => {
            println!("{}", experiment::run_summary(&output));
            if output.all_passed {
                ExitCode::SUCCESS
            } else {
                eprintln!(
                    "自动检查失败；查看 {} 中的失败操作，然后运行 `cargo run -- replay {}`",
                    output.report_path.display(),
                    output.journal_path.display()
                );
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("实验失败: {error}");
            eprintln!("后续操作:");
            eprintln!("  1. source scripts/env.sh");
            eprintln!("  2. GST_DEBUG=3 cargo run -- run --out /tmp/proxyrecover-debug");
            eprintln!("  3. gst-inspect-1.0 pngdec pngenc videoscale");
            eprintln!(
                "  4. 查看 {} 中的失败处置说明",
                out.join("REPORT.md").display()
            );
            ExitCode::FAILURE
        }
    }
}

fn replay_command(args: &[String]) -> ExitCode {
    let Some(path) = args.get(1) else {
        eprintln!("replay 需要日志路径\n{}", usage());
        return ExitCode::FAILURE;
    };
    let mut progress = |message: &str| eprintln!("[replay] {message}");
    match replay::replay_journal(&PathBuf::from(path), &mut progress) {
        Ok(outcome) if outcome.mismatches.is_empty() => {
            println!(
                "replay: {}/{} 步语义一致; report={}",
                outcome.matched,
                outcome.total,
                outcome.report_path.display()
            );
            ExitCode::SUCCESS
        }
        Ok(outcome) => {
            eprintln!(
                "replay: {}/{} 步一致，{} 处不一致; 详见 {}",
                outcome.matched,
                outcome.total,
                outcome.mismatches.len(),
                outcome.report_path.display()
            );
            for mismatch in outcome.mismatches.iter().take(20) {
                eprintln!("  - {mismatch}");
            }
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("回放失败: {error}");
            eprintln!(
                "后续操作: 删除 {} 后重试；若仍失败，用 `GST_DEBUG=3 cargo run -- run` 复现并检查 REPORT.md",
                PathBuf::from(path)
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join("replay")
                    .display()
            );
            ExitCode::FAILURE
        }
    }
}

fn verify_command(args: &[String]) -> ExitCode {
    let Some(path) = args.get(1) else {
        eprintln!("verify-artifacts 需要 manifest 路径\n{}", usage());
        return ExitCode::FAILURE;
    };
    match report::verify_manifest(&PathBuf::from(path)) {
        Ok(count) => {
            println!("verified {count} evidence files: {path}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("证据校验失败: {error}");
            eprintln!("后续操作: 不要信任被修改的证据包；从新目录运行 `cargo run -- run` 重建");
            ExitCode::FAILURE
        }
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}
