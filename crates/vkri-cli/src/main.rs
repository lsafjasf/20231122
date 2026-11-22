//! `vkri` command line interface.
//!
//! Every command is a thin wrapper over `vkri-core::ProjectService`, so the
//! terminal and the GTK4 UI share identical state transitions, audit trails
//! and on-disk artifacts. Long-running extraction installs a Ctrl-C handler
//! that flips a cancellation token: partial evidence is retained and the
//! segment becomes `interrupted`, ready to be resumed on re-entry.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use vkri_core::extract::CancellationToken;
use vkri_core::model::project::{ExtractionConfig, KeyframeMode, SegmentStatus};
use vkri_core::service::{parse_mode, ProjectService};

#[derive(Parser)]
#[command(
    name = "vkri",
    version,
    about = "Video Keyframe Rebuild Index: local correction tool for clock-skewed media fragments"
)]
struct Cli {
    /// Actor label written into audit records.
    #[arg(long, global = true, default_value = "cli")]
    actor: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a new empty project directory.
    Init {
        project_dir: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, default_value_t = String::new())]
        description: String,
    },
    /// List devices, segments and comparisons of a project.
    List { project_dir: PathBuf },
    /// Register a recording device.
    AddDevice {
        project_dir: PathBuf,
        #[arg(long)]
        label: String,
        #[arg(long, default_value_t = String::new())]
        notes: String,
    },
    /// Set the clock offset (nanoseconds) mapped onto the reference timeline.
    SetOffset {
        project_dir: PathBuf,
        #[arg(long)]
        device_id: String,
        #[arg(long)]
        offset_ns: i64,
        #[arg(long, default_value_t = 0)]
        uncertainty_ns: i64,
    },
    /// Register a media fragment (hash + probe, no decoding yet).
    Register {
        project_dir: PathBuf,
        #[arg(long)]
        device_id: String,
        #[arg(long)]
        label: String,
        #[arg(long)]
        source: PathBuf,
        /// Device wall-clock time at PTS zero, Unix nanoseconds.
        #[arg(long)]
        clock_origin_unix_ns: i64,
        /// Group id of the repeated object shared by both devices.
        #[arg(long)]
        group: String,
        #[arg(long, default_value = "interval")]
        mode: String,
        #[arg(long, default_value_t = 1_000_000_000)]
        interval_ns: i64,
        #[arg(long, default_value_t = 480)]
        thumbnail_max_edge: u32,
        #[arg(long, default_value_t = 85)]
        jpeg_quality: u32,
    },
    /// Update extraction configuration of a segment.
    Configure {
        project_dir: PathBuf,
        #[arg(long)]
        segment_id: String,
        #[arg(long)]
        mode: String,
        #[arg(long)]
        interval_ns: i64,
        #[arg(long)]
        thumbnail_max_edge: u32,
        #[arg(long)]
        jpeg_quality: u32,
    },
    /// Mark an extracted segment stale so the next `run` replaces it.
    RequestRerun {
        project_dir: PathBuf,
        #[arg(long)]
        segment_id: String,
    },
    /// Extract keyframes for one segment; press Ctrl-C to stop safely.
    Run {
        project_dir: PathBuf,
        #[arg(long)]
        segment_id: String,
    },
    /// Compare two segments of the same group and suggest a clock offset.
    Compare {
        project_dir: PathBuf,
        #[arg(long)]
        segment_a: String,
        #[arg(long)]
        segment_b: String,
        #[arg(long, default_value_t = 500_000_000)]
        match_window_ns: i64,
        #[arg(long, default_value_t = String::new())]
        notes: String,
    },
    /// Export a self-verifying review bundle.
    Export {
        project_dir: PathBuf,
        #[arg(long)]
        export_root: PathBuf,
        #[arg(long)]
        bundle: String,
    },
    /// Independently verify an exported bundle.
    VerifyExport { bundle_dir: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    vkri_core::media::init_gstreamer().context("failed to initialize GStreamer")?;
    match cli.command {
        Command::Init {
            project_dir,
            name,
            description,
        } => {
            let service = ProjectService::create(&project_dir, &name, &description, &cli.actor)
                .context("failed to create project")?;
            println!(
                "created project '{}' at {}",
                service.project.name,
                project_dir.display()
            );
        }
        Command::List { project_dir } => {
            let service = ProjectService::open(&project_dir, &cli.actor)?;
            print_project(&service);
        }
        Command::AddDevice {
            project_dir,
            label,
            notes,
        } => {
            let mut service = ProjectService::open(&project_dir, &cli.actor)?;
            let device = service.add_device(&label, &notes)?;
            println!("added device {} id={}", device.label, device.id);
        }
        Command::SetOffset {
            project_dir,
            device_id,
            offset_ns,
            uncertainty_ns,
        } => {
            let mut service = ProjectService::open(&project_dir, &cli.actor)?;
            service.set_device_offset(&device_id, offset_ns, uncertainty_ns)?;
            println!("device {device_id} offset set to {offset_ns} ns");
        }
        Command::Register {
            project_dir,
            device_id,
            label,
            source,
            clock_origin_unix_ns,
            group,
            mode,
            interval_ns,
            thumbnail_max_edge,
            jpeg_quality,
        } => {
            let mut service = ProjectService::open(&project_dir, &cli.actor)?;
            let config = build_config(&mode, interval_ns, thumbnail_max_edge, jpeg_quality)?;
            let segment = service.register_segment(
                &device_id,
                &label,
                &source,
                clock_origin_unix_ns,
                &group,
                config,
            )?;
            println!(
                "registered segment {} id={} sha256={}",
                segment.label, segment.id, segment.source_sha256
            );
        }
        Command::Configure {
            project_dir,
            segment_id,
            mode,
            interval_ns,
            thumbnail_max_edge,
            jpeg_quality,
        } => {
            let mut service = ProjectService::open(&project_dir, &cli.actor)?;
            let config = build_config(&mode, interval_ns, thumbnail_max_edge, jpeg_quality)?;
            service.configure_segment(&segment_id, config)?;
            println!("segment {segment_id} configuration updated");
        }
        Command::RequestRerun {
            project_dir,
            segment_id,
        } => {
            let mut service = ProjectService::open(&project_dir, &cli.actor)?;
            service.mark_rerun_requested(&segment_id)?;
            println!("segment {segment_id} marked stale; previous evidence retained");
        }
        Command::Run {
            project_dir,
            segment_id,
        } => {
            let mut service = ProjectService::open(&project_dir, &cli.actor)?;
            let cancel = install_cancel_handler()?;
            let outcome = service
                .run_segment(&segment_id, &cancel, |progress| {
                    eprint!(
                        "\rframes={:>6} keyframes={:>5} pts_ns={:>12}",
                        progress.frames_seen, progress.keyframes_written, progress.pts_ns
                    );
                })
                .context("extraction failed")?;
            eprintln!();
            if outcome.interrupted {
                println!(
                    "INTERRUPTED run={} partial_keyframes={} (resume with the same run command)",
                    outcome.run_id,
                    outcome.keyframes.len()
                );
            } else {
                println!(
                    "completed run={} keyframes={} frames_seen={}",
                    outcome.run_id,
                    outcome.keyframes.len(),
                    outcome.record.frames_seen
                );
            }
        }
        Command::Compare {
            project_dir,
            segment_a,
            segment_b,
            match_window_ns,
            notes,
        } => {
            let mut service = ProjectService::open(&project_dir, &cli.actor)?;
            let comparison = service.compare(&segment_a, &segment_b, match_window_ns, &notes)?;
            println!(
                "comparison {} pairs={} median_delta_ns={:?} suggested_offset_b_ns={:?}",
                comparison.id,
                comparison.entries.len(),
                comparison.median_delta_ns,
                comparison.suggested_offset_b_ns
            );
        }
        Command::Export {
            project_dir,
            export_root,
            bundle,
        } => {
            let service = ProjectService::open(&project_dir, &cli.actor)?;
            let report = service
                .export(&export_root, &bundle)
                .context("export failed")?;
            println!(
                "exported {} files to {} manifest_sha256={}",
                report.file_count,
                report.bundle_dir.display(),
                report.manifest_sha256
            );
        }
        Command::VerifyExport { bundle_dir } => {
            let findings = vkri_core::export::verify_bundle(&bundle_dir)?;
            for finding in findings {
                println!("{finding}");
            }
        }
    }
    Ok(())
}

fn build_config(
    mode: &str,
    interval_ns: i64,
    thumbnail_max_edge: u32,
    jpeg_quality: u32,
) -> Result<ExtractionConfig> {
    let mode: KeyframeMode = parse_mode(mode)?;
    if interval_ns <= 0 {
        bail!("interval_ns must be positive");
    }
    if thumbnail_max_edge == 0 {
        bail!("thumbnail_max_edge must be positive");
    }
    Ok(ExtractionConfig {
        mode,
        interval_ns,
        thumbnail_max_edge,
        jpeg_quality: jpeg_quality.clamp(1, 100),
    })
}

fn install_cancel_handler() -> Result<CancellationToken> {
    let token = CancellationToken::new();
    let handler_token = token.clone();
    ctrlc_handler(handler_token)?;
    Ok(token)
}

fn ctrlc_handler(token: CancellationToken) -> Result<()> {
    ctrlc::set_handler(move || {
        token.cancel();
    })
    .context("failed to install Ctrl-C handler")?;
    Ok(())
}

fn print_project(service: &ProjectService) {
    let project = &service.project;
    println!(
        "project: {} (schema v{})",
        project.name, project.app_info.schema_version
    );
    println!("devices:");
    for device in &project.devices {
        println!(
            "  - {} label={} offset_ns={} ±{}",
            device.id,
            device.label,
            device.clock_offset.offset_ns,
            device.clock_offset.uncertainty_ns
        );
    }
    println!("segments:");
    for segment in &project.segments {
        let status = match segment.status {
            SegmentStatus::Pending => "pending",
            SegmentStatus::Extracted => "extracted",
            SegmentStatus::Interrupted => "interrupted",
            SegmentStatus::Stale => "stale",
        };
        println!(
            "  - {} label={} device={} group={} status={} keyframes={} runs={}",
            segment.id,
            segment.label,
            segment.device_id,
            segment.group_id,
            status,
            segment.keyframes.len(),
            segment.runs.len()
        );
    }
    println!("comparisons: {}", project.comparisons.len());
    for comparison in &project.comparisons {
        println!(
            "  - {} group={} pairs={} median_delta_ns={:?}",
            comparison.id,
            comparison.group_id,
            comparison.entries.len(),
            comparison.median_delta_ns
        );
    }
}
