mod analysis;
mod experiment;
mod gui;
mod journal;
mod pipeline;

use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> &'static str {
    "ditherdiag - audio export dithering diagnosis (replayable experiment)\n\
     usage:\n\
     \x20 ditherdiag run [--out DIR]     execute the experiment, write journal + evidence + report\n\
     \x20 ditherdiag replay <JOURNAL>    re-execute steps from a journal and verify results\n\
     \x20 ditherdiag gui                  launch the GTK4 interface\n\
     \x20 ditherdiag gst-env              record GStreamer environment evidence"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    match cmd {
        "run" => {
            let out = flag_value(&args, "--out")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("artifacts/latest"));
            let mut progress = |msg: &str| eprintln!("[progress] {msg}");
            match experiment::run_experiment(&out, &mut progress) {
                Ok(output) => {
                    print!("{}", experiment::run_summary(&output));
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("experiment failed: {e:#}");
                    ExitCode::FAILURE
                }
            }
        }
        "replay" => match args.get(1) {
            Some(journal) => match experiment::replay_journal(&PathBuf::from(journal)) {
                Ok(true) => {
                    println!("replay: all steps match recorded results");
                    ExitCode::SUCCESS
                }
                Ok(false) => {
                    eprintln!("replay: mismatches detected");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("replay failed: {e:#}");
                    ExitCode::FAILURE
                }
            },
            None => {
                eprintln!("replay requires a journal path\n{}", usage());
                ExitCode::FAILURE
            }
        },
        "gui" => {
            gui::run();
            ExitCode::SUCCESS
        }
        "gst-env" => match pipeline::record_gst_env(&PathBuf::from("artifacts")) {
            Ok(p) => {
                println!("wrote {}", p.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("gst-env failed: {e:#}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
