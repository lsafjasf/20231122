use anyhow::Result;
use credits_layout::{render, report, samples};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("demo") => run_demo(&args[1..]),
        Some("report") => run_reports(&args[1..]),
        Some("ui") | None => credits_layout::ui::run(),
        Some("--help") | Some("-h") | Some("help") => {
            print_help();
            Ok(())
        }
        Some(other) => anyhow::bail!("unknown command: {other} (try `credits-layout help`)"),
    }
}

fn run_reports(args: &[String]) -> Result<()> {
    let out = args.first().map(String::as_str).unwrap_or("artifacts");
    report::export_all_reports(out)
}

fn run_demo(args: &[String]) -> Result<()> {
    let out = args.first().map(String::as_str).unwrap_or("artifacts");
    report::export_all_reports(out)?;
    render::export_all_proofs(out)?;
    report::export_stability_audit(out)?;
    credits_layout::gst::export_demo_video(out, &samples::long_line_sample())?;
    Ok(())
}

fn print_help() {
    println!(
        "credits-layout — 安全区滚动字幕排版工具\n\n\
         credits-layout ui             启动 GTK4 实时预览\n\
         credits-layout report [DIR]   导出三组排版参数与逐行越界 JSON\n\
         credits-layout demo [DIR]     导出报告、调试图和 GStreamer MP4\n\n\
         DIR 默认 artifacts/"
    );
}
