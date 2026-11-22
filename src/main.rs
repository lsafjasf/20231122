use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};

use blackframe_review::analysis::{self, AnalysisRequest};
use blackframe_review::evidence;
use blackframe_review::gst_backend;
use blackframe_review::model::DetectorParams;
use blackframe_review::report;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return;
    }
    if args.is_empty() || args[0] == "--ui" {
        blackframe_review::ui::run();
        return;
    }
    let code = match run_cli(&args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    };
    std::process::exit(code);
}

fn run_cli(args: &[String]) -> Result<(), String> {
    match args[0].as_str() {
        "--analyze" => analyze_cli(args),
        "--inspect" => inspect_cli(args),
        "--demo-bundle" => demo_bundle_cli(args),
        other => Err(format!("unknown command `{other}`; use --help")),
    }
}

fn analyze_cli(args: &[String]) -> Result<(), String> {
    let source = args
        .get(1)
        .ok_or_else(|| "--analyze requires a source path".to_string())?;
    let mut output_root = PathBuf::from("review-evidence");
    let mut params = DetectorParams::default();
    let mut recovered = true;
    let mut note = "CLI 恢复后重放".to_string();
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--output" => {
                output_root = PathBuf::from(
                    args.get(index + 1)
                        .ok_or_else(|| "--output requires a path".to_string())?,
                );
                index += 2;
            }
            "--track" => {
                params.track_index = parse(args, index + 1, "--track")?;
                index += 2;
            }
            "--threshold" => {
                params.luma_threshold = parse(args, index + 1, "--threshold")?;
                index += 2;
            }
            "--min-duration" => {
                params.min_duration_ms = parse(args, index + 1, "--min-duration")?;
                index += 2;
            }
            "--fps" => {
                params.sample_fps = parse(args, index + 1, "--fps")?;
                index += 2;
            }
            "--label" => {
                note = args
                    .get(index + 1)
                    .ok_or_else(|| "--label requires text".to_string())?
                    .clone();
                index += 2;
            }
            "--not-recovered" => {
                recovered = false;
                index += 1;
            }
            unknown => return Err(format!("unknown option `{unknown}`")),
        }
    }
    let request = AnalysisRequest {
        source: PathBuf::from(source),
        params,
        recovered,
        recovery_note: note,
        replayed_from_report: None,
        replay_number: 1,
    };
    let (progress_tx, progress_rx) = mpsc::channel::<blackframe_review::analysis::ProgressUpdate>();
    let progress_thread = std::thread::spawn(move || {
        while let Ok(update) = progress_rx.recv() {
            eprintln!(
                "[{:?}] {:.0}% {}",
                update.stage,
                update.fraction * 100.0,
                update.message
            );
        }
    });
    let result = gst_backend::analyze(&request, Arc::new(AtomicBool::new(false)), progress_tx);
    let _ = progress_thread.join();
    match result {
        Ok(output) => {
            let bundle = evidence::write_bundle(&output_root, output.report, &output.keyframes)
                .map_err(|error| error.to_string())?;
            println!("REPORT={}", bundle.report_path.display());
            println!("REVIEW={}", bundle.markdown_path.display());
            println!("MANIFEST={}", bundle.manifest_path.display());
            println!("EVIDENCE_DIR={}", bundle.root.display());
            Ok(())
        }
        Err(failure) => {
            let mut report = analysis::failure_report(&request, &failure);
            if let Some(record) = report.failure.as_mut() {
                record.evidence_retained = true;
            }
            let bundle = evidence::write_bundle(&output_root, report, &[])
                .map_err(|error| error.to_string())?;
            eprintln!("FAILED {}", failure.code);
            eprintln!("evidence retained at {}", bundle.root.display());
            for (number, action) in failure.actions.iter().enumerate() {
                eprintln!("{}. {} — {}", number + 1, action.label, action.detail);
                if let Some(command) = &action.command {
                    eprintln!("   {command}");
                }
            }
            Err(format!("{}: {}", failure.title, failure.detail))
        }
    }
}

fn inspect_cli(args: &[String]) -> Result<(), String> {
    let path = args
        .get(1)
        .ok_or_else(|| "--inspect requires a report JSON path".to_string())?;
    let report =
        report::read_report(std::path::Path::new(path)).map_err(|error| error.to_string())?;
    println!(
        "{}",
        report::report_to_json(&report).map_err(|error| error.to_string())?
    );
    eprintln!(
        "normalized {} -> {} (migrated={})",
        report.compatibility.input_schema,
        report.compatibility.output_schema,
        report.compatibility.migrated
    );
    Ok(())
}

fn demo_bundle_cli(args: &[String]) -> Result<(), String> {
    let output_root = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("review-evidence"));
    let output = analysis::demo_output();
    let bundle = evidence::write_bundle(&output_root, output.report, &output.keyframes)
        .map_err(|error| error.to_string())?;
    println!("REPORT={}", bundle.report_path.display());
    println!("REVIEW={}", bundle.markdown_path.display());
    Ok(())
}

fn parse<T: std::str::FromStr>(args: &[String], index: usize, option: &str) -> Result<T, String> {
    args.get(index)
        .ok_or_else(|| format!("{option} requires a value"))?
        .parse::<T>()
        .map_err(|_| format!("invalid value for {option}"))
}

fn print_help() {
    println!(
        "blackframe-review — 可复核的镜头黑场检测\n\n\
         UI:\n  blackframe-review [--ui]\n\n\
         Headless:\n  blackframe-review --analyze <file> [--output <dir>] [--track N]\n\
           [--threshold 0..1] [--min-duration MS] [--fps N] [--label TEXT] [--not-recovered]\n\
           blackframe-review --inspect <report.json>\n\
           blackframe-review --demo-bundle [output-dir]\n\n\
         Every analysis keeps report.json, review.md, waveform.csv, key-frame PNGs,\n\
         events.jsonl, source.sha256 and a sealed manifest.json."
    );
}
