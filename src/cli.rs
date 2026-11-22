use crate::fixture::{generate_demo_fixture, load_profile};
use crate::model::RepairConfig;
use crate::session::{ProgressEvent, RunOptions, RunOutcome, Session};
use anyhow::{Context, anyhow};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Debug)]
struct Args {
    fixture: PathBuf,
    workspace: PathBuf,
    pause_ms: Option<u64>,
    memory_kib: u64,
    auto_resume: bool,
    reset: bool,
}

pub fn run_headless(args: &[String]) -> anyhow::Result<()> {
    let args = parse_args(args)?;
    if !args.fixture.join("camera.json").exists() {
        println!("fixture not found; generating deterministic source");
        generate_demo_fixture(&args.fixture)?;
    }
    let profile = load_profile(&args.fixture.join("camera.json"))?;
    if args.reset && args.workspace.exists() {
        fs::remove_dir_all(&args.workspace)
            .with_context(|| format!("reset {}", args.workspace.display()))?;
    }
    let pause_frame = args
        .pause_ms
        .map(|ms| {
            let frame =
                ms.saturating_mul(u64::from(profile.fps_num)) / (1000 * u64::from(profile.fps_den));
            (frame, profile.pts_ns(frame as u32))
        })
        .filter(|(frame, _)| *frame < u64::from(profile.frame_count));
    let mut session = if args.workspace.join("manifest.json").exists() {
        Session::open(&args.workspace)?
    } else {
        let mut config = RepairConfig::demo(profile.clone());
        config.memory_budget_bytes = args.memory_kib.saturating_mul(1024);
        config.pause_before_frame = pause_frame.map(|(frame, _)| frame as u32);
        Session::prepare(&args.fixture, args.workspace.clone(), config)?
    };
    let first = run_once(&mut session, args.memory_kib * 1024, None)?;
    match first {
        RunOutcome::Completed { report, .. } => show_completed(&args, &report),
        RunOutcome::Paused { checkpoint } => {
            println!(
                "PAUSED frame={} pts_ns={} video_offset=0x{:X} metadata_offset=0x{:X} reason={:?}",
                checkpoint.next_frame,
                checkpoint.next_pts_ns,
                checkpoint.next_video_offset,
                checkpoint.next_metadata_offset,
                checkpoint.pause_reason
            );
            if args.auto_resume {
                println!("resuming from atomic checkpoint");
                let second = run_once(&mut session, args.memory_kib * 1024, Some(None))?;
                match second {
                    RunOutcome::Completed { report, .. } => show_completed(&args, &report),
                    other => {
                        println!("unexpected resume outcome: {other:?}");
                        if let RunOutcome::Paused { checkpoint } = other {
                            println!(
                                "still paused at frame {} / pts {}",
                                checkpoint.next_frame, checkpoint.next_pts_ns
                            );
                        }
                    }
                }
            }
        }
        RunOutcome::ResourcePaused { checkpoint, plan } => {
            println!(
                "RESOURCE_PAUSED frame={} pts_ns={} video_offset=0x{:X} metadata_offset=0x{:X}",
                checkpoint.next_frame,
                checkpoint.next_pts_ns,
                checkpoint.next_video_offset,
                checkpoint.next_metadata_offset
            );
            println!(
                "  budget={} bytes estimated={} bytes reason={}",
                plan.budget_bytes,
                plan.estimated_bytes,
                plan.degraded_reason.unwrap_or_default()
            );
            if args.auto_resume {
                println!("raising budget and resuming from the same safe checkpoint");
                let second = run_once(&mut session, 1024 * 1024, Some(None))?;
                match second {
                    RunOutcome::Completed { report, .. } => show_completed(&args, &report),
                    other => {
                        println!("unexpected resource resume outcome: {other:?}");
                    }
                }
            }
        }
    }
    Ok(())
}

fn parse_args(args: &[String]) -> anyhow::Result<Args> {
    let mut parsed = Args {
        fixture: PathBuf::from("fixtures/camera-extreme"),
        workspace: PathBuf::from("artifacts/demo-run"),
        pause_ms: Some(2400),
        memory_kib: 1024,
        auto_resume: false,
        reset: false,
    };
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--fixture" => {
                parsed.fixture = PathBuf::from(next(args, &mut index, "--fixture")?);
            }
            "--workspace" => {
                parsed.workspace = PathBuf::from(next(args, &mut index, "--workspace")?);
            }
            "--pause-ms" => {
                let value: u64 = next(args, &mut index, "--pause-ms")?.parse()?;
                parsed.pause_ms = (value > 0).then_some(value);
            }
            "--memory-kib" => {
                parsed.memory_kib = next(args, &mut index, "--memory-kib")?.parse()?;
            }
            "--auto-resume" => parsed.auto_resume = true,
            "--reset" => parsed.reset = true,
            "--help" | "-h" => {
                println!(
                    "hdr-repair headless [--fixture DIR] [--workspace DIR] [--pause-ms MS]\n\
                     [--memory-kib KIB] [--auto-resume] [--reset]"
                );
                std::process::exit(0);
            }
            other => return Err(anyhow!("unknown argument: {other}")),
        }
        index += 1;
    }
    Ok(parsed)
}

fn next<'a>(args: &'a [String], index: &mut usize, name: &str) -> anyhow::Result<&'a str> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("{name} requires a value"))
}

fn run_once(
    session: &mut Session,
    budget: u64,
    pause_override: Option<Option<u32>>,
) -> anyhow::Result<RunOutcome> {
    let progress = Arc::new(|event: ProgressEvent| match event {
        ProgressEvent::Started {
            first_frame,
            total_frames,
            resumed,
        } => println!(
            "START first_frame={first_frame} total_frames={total_frames} resumed={resumed}"
        ),
        ProgressEvent::Frame {
            frame,
            pts_ns,
            video_offset,
            metadata_offset,
            before,
            after,
            object_present,
            ..
        } => {
            if frame == 72 || object_present {
                println!(
                    "FRAME={frame} PTS_NS={pts_ns} VIDEO_OFFSET=0x{video_offset:X} \
                     META_OFFSET=0x{metadata_offset:X} OBJECT={object_present} \
                     BEFORE_CLL={} AFTER_CLL={}",
                    before.max_cll, after.max_cll
                );
            }
        }
        ProgressEvent::ChunkCommitted {
            next_frame,
            total_frames,
            plan,
        } => println!(
            "CHECKPOINT next_frame={next_frame}/{total_frames} quality={:?}",
            plan.quality
        ),
        ProgressEvent::Paused {
            frame,
            pts_ns,
            video_offset,
            metadata_offset,
            reason,
        } => println!(
            "SAFE_PAUSE frame={frame} pts_ns={pts_ns} video_offset=0x{video_offset:X} \
             metadata_offset=0x{metadata_offset:X} reason={reason:?}"
        ),
        ProgressEvent::ResourcePaused {
            frame,
            pts_ns,
            video_offset,
            metadata_offset,
            plan,
        } => println!(
            "RESOURCE_LOW frame={frame} pts_ns={pts_ns} video_offset=0x{video_offset:X} \
             metadata_offset=0x{metadata_offset:X}: {}",
            plan.degraded_reason.unwrap_or_default()
        ),
        ProgressEvent::Completed {
            acceptance_path,
            output_sha256,
        } => println!(
            "COMPLETE acceptance={} output_sha256={output_sha256}",
            acceptance_path.display()
        ),
    });
    session.run(&RunOptions {
        pause_requested: Arc::new(AtomicBool::new(false)),
        memory_budget_override: Some(budget),
        pause_before_frame_override: pause_override,
        progress: Some(progress),
    })
}

fn show_completed(args: &Args, report: &crate::acceptance::AcceptanceReport) {
    println!(
        "RESULT workspace={} metadata_sha256={} video_sha256={}",
        args.workspace.display(),
        report.output_metadata_sha256,
        report.output_video_sha256
    );
    for check in &report.checks {
        println!(
            "ACCEPT name=\"{}\" pass={} frame={} pts_ns={} video_offset=0x{:X} \
             metadata_offset=0x{:X} before_cll={} after_cll={}",
            check.name,
            check.pass,
            check.frame,
            check.pts_ns,
            check.video_offset,
            check.metadata_offset,
            check.before.max_cll,
            check.after.max_cll
        );
    }
    if let Some(comparison) = report.resume_comparison.as_ref() {
        println!(
            "RESUME paused_frame={} paused_pts_ns={} paused_video_offset=0x{:X} \
             paused_metadata_offset=0x{:X} prefix_verified={}",
            comparison.paused_frame,
            comparison.paused_pts_ns,
            comparison.paused_video_offset,
            comparison.paused_metadata_offset,
            comparison.prefix_verified
        );
    }
    println!(
        "EVIDENCE {}/evidence/events.jsonl {}/acceptance.json",
        args.workspace.display(),
        args.workspace.display()
    );
}
