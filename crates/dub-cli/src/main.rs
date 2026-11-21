//! dub-cli: drive and independently review the pausable dub-switch flow.
//!
//!   dub-cli make-sample --out sample.mkv
//!   dub-cli start --input sample.mkv --workdir work --languages eng,zho,eng [--pause-after N]
//!   dub-cli resume --workdir work [--pause-after N]
//!   dub-cli report --workdir work
//!   dub-cli verify --workdir work
//!
//! Degradation can be simulated with --simulate-low-disk /
//! --simulate-missing-encoder on start/resume.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context};
use dub_core::degrade::Policy;
use dub_core::pipeline::{run_job, Outcome, RunOptions};
use dub_core::{media, report, state, DubJob};

#[derive(Default)]
struct Args {
    positional: Vec<String>,
    out: Option<PathBuf>,
    input: Option<PathBuf>,
    workdir: Option<PathBuf>,
    job_id: Option<String>,
    languages: Option<String>,
    pause_after: Option<u32>,
    simulate_low_disk: bool,
    simulate_missing_encoder: bool,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--out" => a.out = Some(it.next().context("--out needs a value")?.into()),
            "--input" => a.input = Some(it.next().context("--input needs a value")?.into()),
            "--workdir" => a.workdir = Some(it.next().context("--workdir needs a value")?.into()),
            "--job-id" => a.job_id = Some(it.next().context("--job-id needs a value")?),
            "--languages" => a.languages = Some(it.next().context("--languages needs a value")?),
            "--pause-after" => {
                a.pause_after = Some(it.next().context("--pause-after needs a value")?.parse()?)
            }
            "--simulate-low-disk" => a.simulate_low_disk = true,
            "--simulate-missing-encoder" => a.simulate_missing_encoder = true,
            other if other.starts_with("--") => bail!("unknown flag {other}"),
            pos => a.positional.push(pos.to_string()),
        }
    }
    Ok(a)
}

fn policy_from(a: &Args) -> Policy {
    Policy {
        simulate_low_disk: a.simulate_low_disk,
        simulate_missing_encoder: a.simulate_missing_encoder,
        ..Policy::default()
    }
}

fn run_with_options(workdir: &Path, a: &Args) -> anyhow::Result<Outcome> {
    let remaining = Arc::new(AtomicU32::new(a.pause_after.unwrap_or(u32::MAX)));
    let flag = remaining.clone();
    let opts = RunOptions {
        policy: policy_from(a),
        should_pause: Box::new(move || {
            // Pause once the requested number of segments has been done.
            flag.load(Ordering::SeqCst) == 0
        }),
        on_progress: Box::new(move |done, total| {
            remaining.fetch_sub(1, Ordering::SeqCst);
            println!("progress: {done}/{total} segments");
        }),
    };
    run_job(workdir, &opts)
}

fn main() -> anyhow::Result<ExitCode> {
    gstreamer::init()?;
    let a = parse_args()?;
    let Some(cmd) = a.positional.first().cloned() else {
        eprintln!("usage: dub-cli <make-sample|start|resume|report|verify> [flags]");
        return Ok(ExitCode::from(2));
    };
    match cmd.as_str() {
        "make-sample" => {
            let out = a.out.clone().unwrap_or_else(|| "sample.mkv".into());
            media::make_sample(&out)?;
            println!("sample written to {}", out.display());
            for (pad, lang) in media::list_audio_tracks(&out)? {
                println!("  track {pad}: language={lang}");
            }
        }
        "start" => {
            let input = a.input.clone().context("start needs --input")?;
            let workdir = a.workdir.clone().context("start needs --workdir")?;
            let languages: Vec<String> = a
                .languages
                .clone()
                .context("start needs --languages eng,zho,...")?
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            let total_ms = media::duration_ms(&input)?;
            let job_id = a.job_id.clone().unwrap_or_else(|| "job-1".into());
            let out_dir = workdir.join("segments");
            std::fs::create_dir_all(&out_dir)?;
            let job = DubJob::new(
                &job_id,
                &input.canonicalize()?.to_string_lossy(),
                &out_dir.to_string_lossy(),
                total_ms,
                &languages,
            )?;
            state::save_state(&workdir, &job)?;
            println!(
                "job {job_id}: {} segments over {total_ms} ms",
                languages.len()
            );
            match run_with_options(&workdir, &a)? {
                Outcome::Completed => println!("completed"),
                Outcome::Paused => println!(
                    "paused; resume with: dub-cli resume --workdir {}",
                    workdir.display()
                ),
            }
        }
        "resume" => {
            let workdir = a.workdir.clone().context("resume needs --workdir")?;
            match run_with_options(&workdir, &a)? {
                Outcome::Completed => println!("completed"),
                Outcome::Paused => println!("paused again; resume to continue"),
            }
        }
        "report" => {
            let workdir = a.workdir.clone().context("report needs --workdir")?;
            let path = report::export_report(&workdir)?;
            println!("report written to {}", path.display());
        }
        "verify" => {
            let workdir = a.workdir.clone().context("verify needs --workdir")?;
            let review = report::verify_workdir(&workdir)?;
            for (name, ok, detail) in &review.checks {
                println!("  [{}] {name}: {detail}", if *ok { "ok" } else { "FAIL" });
            }
            println!("overall: {}", if review.ok { "OK" } else { "FAILED" });
            return Ok(if review.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        other => bail!("unknown command {other}"),
    }
    Ok(ExitCode::SUCCESS)
}
