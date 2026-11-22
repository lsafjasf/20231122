use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use color_sample_flow::acceptance;
use color_sample_flow::checkpoint;
use color_sample_flow::compare;
use color_sample_flow::demo;
use color_sample_flow::evidence::EvidenceLog;
use color_sample_flow::fixture;
use color_sample_flow::model::{ExtremeParams, SourceInfo};
use color_sample_flow::plan::{build_demo_plan, validate_demo_plan};
use color_sample_flow::runner::{self, RunConfig, RunOutcome};

fn main() {
    if let Err(error) = real_main() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let workspace = std::env::current_dir()?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("demo");
    match command {
        "demo" => {
            let pass = demo::run_demo(&workspace)?;
            print_demo_summary(&workspace)?;
            if !pass {
                bail!("acceptance failed; see artifacts/acceptance.json");
            }
        }
        "fixtures" => {
            let (source_path, manifest_path) = fixture::default_paths();
            let info = fixture::ensure_original(
                &workspace.join(source_path),
                &workspace.join(manifest_path),
            )?;
            println!("{}", serde_json::to_string_pretty(&info)?);
        }
        "run" => run_command(&workspace, &args[1..])?,
        "verify" => {
            let pass = verify_existing(&workspace)?;
            if !pass {
                bail!("acceptance failed; see artifacts/acceptance.json");
            }
        }
        "ui" => {
            color_sample_flow::ui::run();
        }
        "-h" | "--help" | "help" => print_help(),
        other => bail!("unknown command '{other}'; use --help"),
    }
    Ok(())
}

fn run_command(workspace: &Path, args: &[String]) -> Result<()> {
    let mut source_path = workspace.join(fixture::SOURCE_PATH);
    let mut out_dir = workspace.join("artifacts/run");
    let mut resume = false;
    let mut pause_after = Some(ExtremeParams::demo().pause_after);
    let mut params = ExtremeParams::demo();

    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                source_path = PathBuf::from(take_value(args, &mut index, "--source")?);
            }
            "--out" => {
                out_dir = PathBuf::from(take_value(args, &mut index, "--out")?);
            }
            "--resume" => resume = true,
            "--pause-after" => {
                pause_after = Some(
                    take_value(args, &mut index, "--pause-after")?
                        .parse()
                        .context("--pause-after must be an integer")?,
                );
            }
            "--no-pause" => pause_after = None,
            "--mem-budget" => {
                params.memory_budget_bytes = take_value(args, &mut index, "--mem-budget")?
                    .parse()
                    .context("--mem-budget must be bytes")?;
            }
            "--scale" => {
                params.requested_scale_percent = take_value(args, &mut index, "--scale")?
                    .parse()
                    .context("--scale must be a percentage")?;
            }
            other => bail!("unknown run option '{other}'"),
        }
        index += 1;
    }

    let source = if source_path.ends_with(fixture::SOURCE_PATH) {
        fixture::ensure_original(&source_path, &workspace.join(fixture::MANIFEST_PATH))?
    } else {
        SourceInfo::from_path(&source_path)?
    };
    let params = ExtremeParams {
        pause_after: pause_after.unwrap_or(0),
        ..params
    };
    let plan = build_demo_plan(source, params);
    validate_demo_plan(&plan)?;
    std::fs::create_dir_all(&out_dir)?;
    let evidence_path = out_dir.join("evidence.jsonl");
    let mut evidence = if resume {
        EvidenceLog::open_append(&evidence_path)?
    } else {
        EvidenceLog::create(&evidence_path)?
    };
    let config = RunConfig {
        out_dir: out_dir.clone(),
        resume,
        pause_after,
        pause_flag: None,
    };
    let outcome = runner::run(&plan, &config, &mut evidence, |event| {
        eprintln!("{}", serde_json::to_string(&event).unwrap_or_default());
    })?;
    let cp = outcome.checkpoint();
    runner::write_results(&out_dir, cp)?;
    match outcome {
        RunOutcome::Paused(_) => println!(
            "paused at {}/{}; resume with --resume",
            cp.next_index, cp.total
        ),
        RunOutcome::Completed(_) => {
            println!("completed {} samples in {}", cp.total, out_dir.display())
        }
    }
    if let Some(first) = cp.results.first() {
        println!(
            "first anchor: t={} ms raw_offset={} rgb={:?}",
            first.timestamp_ms, first.raw_offset, first.rgb
        );
    }
    if let Some(last) = cp.results.last() {
        println!(
            "last anchor: t={} ms raw_offset={} rgb={:?}",
            last.timestamp_ms, last.raw_offset, last.rgb
        );
    }
    Ok(())
}

fn take_value<'a>(args: &'a [String], index: &mut usize, name: &str) -> Result<&'a str> {
    *index += 1;
    args.get(*index)
        .map(String::as_str)
        .with_context(|| format!("{name} requires a value"))
}

fn verify_existing(workspace: &Path) -> Result<bool> {
    let artifacts = workspace.join("artifacts");
    let plan = demo::make_plan(workspace)?;
    let results = demo::read_results(artifacts.join("results.json"))?;
    let fresh = demo::read_results(artifacts.join("results.fresh.json"))?;
    let paused = demo::read_results(artifacts.join("results.paused.json"))?;
    let cp = checkpoint::load(&artifacts)?;
    let mut prefix = compare::compare_results(&paused, &results[..paused.len().min(results.len())]);
    prefix.pause_resume_equivalent = prefix.mismatch_count == 0 && paused.len() <= results.len();
    prefix.prefix_preserved = prefix.pause_resume_equivalent;
    let mut report = compare::compare_results(&results, &fresh);
    report.prefix_preserved = prefix.pause_resume_equivalent;
    let acceptance = acceptance::verify(
        &plan,
        &results,
        &fresh,
        &report,
        &artifacts.join("evidence.jsonl"),
        &workspace.join(fixture::MANIFEST_PATH),
        cp.scale_percent,
    )?;
    acceptance::write(&artifacts.join("acceptance.json"), &acceptance)?;
    println!("{}", serde_json::to_string_pretty(&acceptance)?);
    Ok(acceptance.all_ok())
}

fn print_demo_summary(workspace: &Path) -> Result<()> {
    let summary_path = workspace.join("artifacts/summary.json");
    if summary_path.exists() {
        println!("{}", std::fs::read_to_string(summary_path)?);
    }
    let acceptance_path = workspace.join("artifacts/acceptance.json");
    if acceptance_path.exists() {
        println!("{}", std::fs::read_to_string(acceptance_path)?);
    }
    Ok(())
}

fn print_help() {
    println!(
        "color-sample-flow {}\n\
         \n\
         Commands:\n\
           demo       pause at 8/25, resume, run fresh reference, compare and verify\n\
           run        process a video (options: --source --out --resume --pause-after N --mem-budget N --scale N)\n\
           verify     re-check existing artifacts without processing video\n\
           fixtures   ensure originals/source.mkv exists and verify its manifest\n\
           ui         open the GTK4 control window\n",
        color_sample_flow::APP_VERSION
    );
}
