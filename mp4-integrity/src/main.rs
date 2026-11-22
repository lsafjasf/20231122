//! MP4/ISO-BMFF container integrity checker.
//!
//! Sub-commands:
//!   check <file>            walk boxes, print offsets/owners/errors, reload
//!                           via GStreamer and print the post-reload count
//!   repair <in> --out <out> conservative repair, always writes a new file
//!   gui                     GTK4 front-end

use mp4_integrity_lib::{gst_check, gui, isobmff, repair, report_view};

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mp4-integrity", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect a container and reload it through GStreamer.
    Check {
        /// MP4/ISO-BMFF file to inspect.
        file: PathBuf,
        /// Skip the GStreamer reload step.
        #[arg(long)]
        no_reload: bool,
    },
    /// Repair a container into a brand-new file (original is never modified).
    Repair {
        /// Damaged input container.
        input: PathBuf,
        /// Destination path; must not exist and must differ from the input.
        #[arg(long, short)]
        out: PathBuf,
        /// Reload the repaired file via GStreamer after writing it.
        #[arg(long)]
        reload: bool,
    },
    /// Launch the GTK4 graphical interface.
    Gui,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Check { file, no_reload } => cmd_check(&file, !no_reload),
        Command::Repair { input, out, reload } => cmd_repair(&input, &out, reload),
        Command::Gui => gui::run(),
    }
}

fn cmd_check(path: &std::path::Path, reload: bool) -> anyhow::Result<()> {
    let report = isobmff::inspect_file(path)?;
    print!("{}", report_view::render_summary(&report, path));
    println!();
    print!("{}", report_view::render_report_table(&report));
    println!();
    print!("{}", report_view::render_errors(&report));

    if reload {
        println!();
        match gst_check::reload_and_count(path) {
            Ok(result) => {
                println!(
                    "GStreamer reload: success={} | tracks after reload: {} [{}]",
                    result.success,
                    result.track_count,
                    result.track_kinds.join(", ")
                );
                if let Some(msg) = result.error_message {
                    println!("  demuxer message: {msg}");
                }
            }
            Err(e) => println!("GStreamer reload failed to start: {e}"),
        }
    }
    Ok(())
}

fn cmd_repair(input: &std::path::Path, out: &std::path::Path, reload: bool) -> anyhow::Result<()> {
    match repair::repair_file(input, out) {
        Ok(outcome) => {
            println!("repair OK");
            println!("  output: {}", outcome.output.display());
            println!("  bytes written: {}", outcome.bytes_written);
            println!("  dropped boxes: {}", outcome.dropped_boxes.join(", "));
            println!("  trimmed tail: {}", outcome.trimmed_tail);
            let repaired_report = isobmff::inspect_file(&outcome.output)?;
            println!(
                "  structural errors after repair: {}",
                repaired_report.errors.len()
            );
            println!(
                "  first error offset after repair: {}",
                match repaired_report.first_error_offset() {
                    Some(o) => o.to_string(),
                    None => "none".to_string(),
                }
            );
            if reload {
                let result = gst_check::reload_and_count(&outcome.output)?;
                println!(
                    "  GStreamer tracks after reload: {} [{}] (success={})",
                    result.track_count,
                    result.track_kinds.join(", "),
                    result.success
                );
            }
            Ok(())
        }
        Err(e) => {
            eprintln!("repair refused: {e}");
            std::process::exit(2);
        }
    }
}
