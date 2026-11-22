//! 音频片段淡化检查 —— 来源检查页。
//!
//! 技术栈固定为 Rust + GStreamer + GTK4：
//! - GStreamer 负责解码/重采样并留下处理步骤；
//! - GTK4 负责来源检查页的三段式溯源展示；
//! - 无头模式（`--batch`）用于自动化验证与报告留档。

mod ui;

use fadecheck::decoder_public::init as gst_init;
use fadecheck::engine_public::run_batch;

fn print_usage() {
    eprintln!(
        "用法:\n  fadecheck                      启动 GTK4 来源检查页\n  fadecheck --batch <路径>...    无头批处理，输出 JSON 报告到 stdout"
    );
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("--help")
        || args.first().map(|s| s.as_str()) == Some("-h")
    {
        print_usage();
        return glib::ExitCode::SUCCESS;
    }

    if let Some(rest) = args
        .split_first()
        .filter(|(head, _)| head.as_str() == "--batch")
    {
        let paths = rest.1.to_vec();
        if paths.is_empty() {
            print_usage();
            return glib::ExitCode::FAILURE;
        }
        return run_headless(&paths);
    }

    if !args.is_empty() {
        print_usage();
        return glib::ExitCode::FAILURE;
    }

    if let Err(message) = gst_init() {
        eprintln!("{message}");
        return glib::ExitCode::FAILURE;
    }
    let code = ui::run();
    // GTK 退出后不存在残留临时目录：工作线程已显式 cleanup，Drop 再兜底。
    code
}

fn run_headless(paths: &[String]) -> glib::ExitCode {
    if let Err(message) = gst_init() {
        eprintln!("{message}");
        return glib::ExitCode::FAILURE;
    }
    let (report, temp) = run_batch(paths);
    match serde_json::to_string_pretty(&report) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("报告序列化失败: {e}");
            temp.cleanup();
            return glib::ExitCode::FAILURE;
        }
    }
    temp.cleanup();
    if temp.path().exists() {
        eprintln!("警告: 临时目录未能删除 {}", temp.path().display());
        glib::ExitCode::FAILURE
    } else {
        glib::ExitCode::SUCCESS
    }
}
