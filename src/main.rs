use std::path::PathBuf;
use std::process::ExitCode;

use alc_review::evidence::{self, AssessOptions, AssessmentOutcome};
use alc_review::gst_audio;
use alc_review::report::Verdict;
use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "alc-review",
    version,
    about = "可复核的音频丢包掩蔽评估（Rust + GStreamer + GTK4）"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 运行自动评估并生成不可混淆的自动/人工双栏证据包。
    Assess(AssessArgs),
    /// 校验证据包中的文件哈希与清单。
    Verify(VerifyArgs),
    /// 打开 GTK4 回放与人工确认界面。
    Gui(GuiArgs),
}

#[derive(Debug, Args, Clone)]
struct AssessArgs {
    #[arg(long)]
    manifest: Option<PathBuf>,
    #[arg(long)]
    audio: Option<PathBuf>,
    #[arg(long)]
    out: Option<PathBuf>,
    #[arg(long)]
    no_copy_audio: bool,
    #[arg(long)]
    force: bool,
    #[arg(long)]
    max_seconds: Option<u64>,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    #[arg(long)]
    evidence: PathBuf,
}

#[derive(Debug, Args, Clone)]
struct GuiArgs {
    #[arg(long)]
    manifest: Option<PathBuf>,
    #[arg(long)]
    audio: Option<PathBuf>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Gui(GuiArgs {
        manifest: None,
        audio: None,
    })) {
        Command::Assess(args) => run_assess(&args),
        Command::Verify(args) => run_verify(&args),
        Command::Gui(args) => {
            let code = alc_review::gui::run(args.manifest, args.audio);
            ExitCode::from(u8::from(code))
        }
    }
}

fn run_assess(args: &AssessArgs) -> ExitCode {
    let options = AssessOptions {
        manifest: args.manifest.clone(),
        audio: args.audio.clone(),
        out: args.out.clone(),
        copy_audio: !args.no_copy_audio,
        force: args.force,
        max_seconds: args.max_seconds.unwrap_or_else(gst_audio::max_seconds),
    };
    match evidence::assess(&options) {
        Ok(outcome) => {
            print_outcome(&outcome);
            if outcome.success {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(failure) => {
            eprintln!("错误：{failure}");
            eprintln!("后续操作：{}", failure.action);
            ExitCode::from(1)
        }
    }
}

fn print_outcome(outcome: &AssessmentOutcome) {
    println!("案件：{}", outcome.case_id);
    println!("证据包：{}", outcome.evidence_dir.display());
    if let Some(automated) = &outcome.automated {
        println!("自动结论：{}", automated.verdict.label());
        println!("摘要：{}", automated.summary);
        if automated.verdict != Verdict::Pass {
            for action in automated.next_actions() {
                println!("后续操作：{action}");
            }
        }
    }
    if let Some(manual) = &outcome.manual {
        println!("人工确认：{}（与自动结论分栏保存）", manual.status.label());
    }
    println!("报告：{}", outcome.evidence_dir.join("report.md").display());
    if let Some(failure) = &outcome.failure {
        println!("失败：{}", failure.message);
        println!("后续操作：{}", failure.action);
    }
}

fn run_verify(args: &VerifyArgs) -> ExitCode {
    match evidence::verify_evidence(&args.evidence) {
        Ok(report) => {
            println!("证据目录：{}", report.root.display());
            println!("校验文件：{}", report.checked);
            if report.ok() {
                println!("结果：通过，所有已记录哈希一致。");
                ExitCode::SUCCESS
            } else {
                println!(
                    "结果：失败，{} 个文件缺失，{} 个哈希不一致。",
                    report.missing, report.mismatched
                );
                for issue in &report.issues {
                    println!("- {issue}");
                }
                eprintln!(
                    "后续操作：不要修改现有证据；重新采集输入并生成新的证据包，再比对两次的 evidence-index.json。"
                );
                ExitCode::from(1)
            }
        }
        Err(failure) => {
            eprintln!("错误：{failure}");
            eprintln!("后续操作：{}", failure.action);
            ExitCode::from(1)
        }
    }
}
