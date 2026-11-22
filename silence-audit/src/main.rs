//! 命令行入口：`batch` 运行可复核批处理，`review` 打开 GTK4 复核窗口。

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use gtk4::prelude::*;
use silence_audit::model::{ObjectResult, SilenceParams};
use silence_audit::runner;

#[derive(Parser)]
#[command(name = "silence-audit", version, about = "音轨静音段识别可复核流程")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 解析快照并批量识别静音段，输出报告与波形证据。
    Batch {
        /// 快照 JSON 路径（legacy 或 v2 格式均可）。
        #[arg(long)]
        snapshot: PathBuf,
        /// 运行输出根目录。
        #[arg(long, default_value = "runs")]
        output: PathBuf,
        /// 快照中相对媒体路径的解析基准目录。
        #[arg(long)]
        media_base: Option<PathBuf>,
        /// 静音判定阈值（dB）。
        #[arg(long, default_value_t = -50.0)]
        threshold_db: f64,
        /// 进入静音的持续时间（毫秒）。
        #[arg(long, default_value_t = 200)]
        on_millis: u64,
        /// 退出静音的持续时间（毫秒）。
        #[arg(long, default_value_t = 300)]
        off_millis: u64,
        /// level 消息间隔（毫秒）。
        #[arg(long, default_value_t = 50)]
        interval_millis: u64,
        /// 重采样率（Hz）。
        #[arg(long, default_value_t = 48_000)]
        resample_rate: u32,
        /// 波形桶数。
        #[arg(long, default_value_t = 512)]
        waveform_buckets: usize,
    },
    /// 读取某次运行的 report.json 并打开 GTK4 复核窗口。
    Review {
        /// 运行目录（含 report.json）。
        #[arg(long)]
        run_dir: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Batch {
            snapshot,
            output,
            media_base,
            threshold_db,
            on_millis,
            off_millis,
            interval_millis,
            resample_rate,
            waveform_buckets,
        } => {
            let params = SilenceParams {
                threshold_db,
                on_millis,
                off_millis,
                interval_millis,
                resample_rate,
                waveform_buckets,
            };
            let out = runner::run_batch(&snapshot, &output, &params, media_base.as_deref())?;
            println!("运行完成: {}", out.run_dir.display());
            println!("报告: {}", out.report_path.display());
            println!(
                "对象 {} 个，成功 {}，失败 {}，报告SHA256 {}",
                out.report.total_objects,
                out.report.succeeded,
                out.report.failed,
                out.report_sha256
            );
            for result in &out.report.results {
                println!(
                    "  - {} [{}] {} 静音段={}",
                    result.object_id,
                    result.source_format,
                    format!("{:?}", result.status).to_lowercase(),
                    result.segments.len()
                );
            }
            Ok(())
        }
        Command::Review { run_dir } => {
            let report_path = run_dir.join("report.json");
            let report: ReviewReport = serde_json::from_slice(&std::fs::read(&report_path)?)?;
            let app = gtk4::Application::builder()
                .application_id("org.audit.silence.review")
                .build();
            app.connect_activate(move |app| {
                silence_audit::ui::run_review_window(app, report.results.clone());
            });
            // 只传程序名，避免 GApplication 重新解析已由 clap 消费的参数。
            let program = std::env::args()
                .next()
                .unwrap_or_else(|| "silence-audit".to_string());
            app.run_with_args(&[program]);
            Ok(())
        }
    }
}

#[derive(serde::Deserialize)]
struct ReviewReport {
    #[serde(default)]
    results: Vec<ObjectResult>,
}
