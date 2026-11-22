use crate::journal::{self, Journal, StepResult};
use crate::{objects, pipeline, snapshot};
use serde_json::json;
use std::error::Error;
use std::path::{Path, PathBuf};

type RunResult<T> = Result<T, Box<dyn Error>>;

struct Ctx {
    journal: Journal,
    evidence_dir: PathBuf,
    temp_dir: PathBuf,
    processed: Vec<String>,
    failed: Vec<String>,
    duplicates: u64,
}

impl Ctx {
    fn state(&self) -> serde_json::Value {
        json!({
            "processed": self.processed,
            "failed": self.failed,
            "duplicates": self.duplicates,
        })
    }

    fn evidence(&self, name: &str, contents: &str) -> std::io::Result<PathBuf> {
        let path = self.evidence_dir.join(name);
        std::fs::write(&path, contents)?;
        Ok(path)
    }
}

fn run_id() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("run-{secs}")
}

pub fn run() -> RunResult<()> {
    let id = run_id();
    let evidence_dir = Path::new("artifacts").join(&id);
    let temp_dir = Path::new("tmp").join(&id);
    std::fs::create_dir_all(&temp_dir)?;
    let mut ctx = Ctx {
        journal: Journal::create(&evidence_dir.join("journal.jsonl"))?,
        evidence_dir: evidence_dir.clone(),
        temp_dir: temp_dir.clone(),
        processed: Vec::new(),
        failed: Vec::new(),
        duplicates: 0,
    };
    println!("experiment {id}: evidence -> {}", evidence_dir.display());

    step_load_snapshots(&mut ctx)?;
    step_ingest_objects(&mut ctx)?;
    step_verify_duplicates(&mut ctx)?;
    step_verify_order(&mut ctx)?;
    step_run_pipelines(&mut ctx)?;
    step_report(&mut ctx)?;
    step_cleanup(&mut ctx)?;

    println!(
        "done: {} processed, {} failed, {} duplicate(s); evidence kept, temp cleaned",
        ctx.processed.len(),
        ctx.failed.len(),
        ctx.duplicates
    );
    Ok(())
}

fn step_load_snapshots(ctx: &mut Ctx) -> RunResult<()> {
    let v1 = snapshot::load(Path::new("snapshots/v1.json"))?;
    let v2 = snapshot::load(Path::new("snapshots/v2.json"))?;
    let diff = snapshot::diff(&v1, &v2);
    ctx.evidence("snapshot-diff.json", &serde_json::to_string_pretty(&diff)?)?;
    ctx.journal.record(
        "load-snapshots",
        json!({ "files": ["snapshots/v1.json", "snapshots/v2.json"] }),
        ctx.state(),
        StepResult::Ok {
            output: json!({ "v1": v1.version, "v2": v2.version, "diff": diff }),
        },
    )?;
    Ok(())
}

fn step_ingest_objects(ctx: &mut Ctx) -> RunResult<()> {
    for path in objects::list(Path::new("objects")) {
        let input = json!({ "path": path.display().to_string() });
        if let Some(original) = ctx.journal.is_duplicate(&input) {
            ctx.duplicates += 1;
            ctx.journal.record(
                "ingest-object",
                input,
                ctx.state(),
                StepResult::Duplicate {
                    original_seq: original,
                },
            )?;
            continue;
        }
        // An exception on one object (e.g. unknown extension) is recorded
        // and the loop continues with the remaining objects.
        let result = match objects::load(&path) {
            Ok(obj) => {
                ctx.processed.push(obj.name.clone());
                StepResult::Ok {
                    output: json!({
                        "name": obj.name,
                        "kind": obj.kind,
                        "sha256": obj.source_sha256,
                    }),
                }
            }
            Err(err) => {
                ctx.failed.push(path.display().to_string());
                StepResult::Failed {
                    error: err.to_string(),
                }
            }
        };
        ctx.journal
            .record("ingest-object", input, ctx.state(), result)?;
    }
    Ok(())
}

/// Re-submit the first object: the journal must flag it as a duplicate
/// instead of re-ingesting it.
fn step_verify_duplicates(ctx: &mut Ctx) -> RunResult<()> {
    let first = objects::list(Path::new("objects")).into_iter().next();
    let Some(path) = first else { return Ok(()) };
    let input = json!({ "path": path.display().to_string() });
    let result = match ctx.journal.is_duplicate(&input) {
        Some(original) => {
            ctx.duplicates += 1;
            StepResult::Duplicate {
                original_seq: original,
            }
        }
        None => StepResult::Failed {
            error: "duplicate submission was not detected".into(),
        },
    };
    ctx.journal
        .record("verify-duplicates", input, ctx.state(), result)?;
    Ok(())
}

/// The final processed-set digest must not depend on submission order:
/// process the object list forwards and in reverse and compare digests.
fn step_verify_order(ctx: &mut Ctx) -> RunResult<()> {
    let paths = objects::list(Path::new("objects"));
    let digest_of = |ordered: &[PathBuf]| -> String {
        let mut names: Vec<String> = Vec::new();
        for path in ordered {
            if let Ok(obj) = objects::load(path) {
                names.push(obj.name);
            }
        }
        names.sort();
        journal::sha256_hex(names.join("|").as_bytes())
    };
    let forward = digest_of(&paths);
    let mut reversed = paths.clone();
    reversed.reverse();
    let backward = digest_of(&reversed);
    let deterministic = forward == backward;
    ctx.evidence(
        "order-verification.json",
        &serde_json::to_string_pretty(&json!({
            "forward_digest": forward,
            "reversed_digest": backward,
            "deterministic": deterministic,
        }))?,
    )?;
    ctx.journal.record(
        "verify-order",
        json!({ "objects": paths.len(), "reversed": true }),
        ctx.state(),
        if deterministic {
            StepResult::Ok {
                output: json!({ "digest": forward }),
            }
        } else {
            StepResult::Failed {
                error: "order changed the resulting state digest".into(),
            }
        },
    )?;
    Ok(())
}

/// Run both control snapshots through the low-latency pipeline and keep
/// per-buffer latency samples as evidence.
fn step_run_pipelines(ctx: &mut Ctx) -> RunResult<()> {
    for file in ["snapshots/v1.json", "snapshots/v2.json"] {
        let snap = snapshot::load(Path::new(file))?;
        let input = json!({ "snapshot": file, "pipeline": snap.pipeline });
        let result = match pipeline::run_monitored(&snap.pipeline, snap.duration_ms) {
            Ok(out) => {
                let csv_name = format!("latency-{}.csv", snap.version);
                ctx.evidence(&csv_name, &out.samples_csv)?;
                let within = out.stats.mean_ms <= snap.expect.max_mean_latency_ms;
                StepResult::Ok {
                    output: json!({
                        "version": snap.version,
                        "stats": out.stats,
                        "csv": csv_name,
                        "within_budget": within,
                    }),
                }
            }
            Err(err) => StepResult::Failed { error: err },
        };
        ctx.journal
            .record("run-pipeline", input, ctx.state(), result)?;
    }
    Ok(())
}

fn step_report(ctx: &mut Ctx) -> RunResult<()> {
    let report = json!({
        "processed": ctx.processed,
        "failed": ctx.failed,
        "duplicates": ctx.duplicates,
        "journal": ctx.journal.path.display().to_string(),
    });
    ctx.evidence("report.json", &serde_json::to_string_pretty(&report)?)?;
    ctx.journal.record(
        "report",
        json!({ "path": "report.json" }),
        ctx.state(),
        StepResult::Ok {
            output: json!({ "written": true }),
        },
    )?;
    Ok(())
}

/// Remove temporary state at the end of the experiment; evidence under
/// artifacts/ is kept.
fn step_cleanup(ctx: &mut Ctx) -> RunResult<()> {
    let removed = ctx.temp_dir.exists();
    if removed {
        std::fs::remove_dir_all(&ctx.temp_dir)?;
    }
    if Path::new("tmp").exists() && std::fs::read_dir("tmp")?.next().is_none() {
        std::fs::remove_dir("tmp")?;
    }
    ctx.journal.record(
        "cleanup",
        json!({ "temp_dir": ctx.temp_dir.display().to_string() }),
        ctx.state(),
        StepResult::Ok {
            output: json!({ "removed": removed }),
        },
    )?;
    Ok(())
}

/// Re-execute a journal: verify every recorded hash and re-run the
/// deterministic steps (object ingestion), comparing outputs.
pub fn replay(journal_path: &str) -> RunResult<()> {
    let records = journal::read_all(Path::new(journal_path))?;
    let mut checked = 0u64;
    let mut mismatches = 0u64;
    for record in &records {
        if record.input_sha256 != journal::sha256_json(&record.input) {
            println!("seq {}: input hash mismatch", record.seq);
            mismatches += 1;
        }
        if record.result_sha256 != journal::sha256_json(&serde_json::to_value(&record.result)?) {
            println!("seq {}: result hash mismatch", record.seq);
            mismatches += 1;
        }
        if record.step == "ingest-object" {
            if let Some(path) = record.input.get("path").and_then(|p| p.as_str()) {
                let replayed = objects::load(Path::new(path));
                let matches = match (&record.result, replayed) {
                    (StepResult::Ok { output }, Ok(obj)) => {
                        output.get("sha256").and_then(|s| s.as_str())
                            == Some(obj.source_sha256.as_str())
                    }
                    (StepResult::Failed { .. }, Err(_)) => true,
                    _ => false,
                };
                if !matches {
                    println!("seq {}: replayed ingestion differs", record.seq);
                    mismatches += 1;
                }
            }
        }
        checked += 1;
    }
    println!("replayed {checked} step(s), {mismatches} mismatch(es)");
    if mismatches > 0 {
        return Err(format!("{mismatches} mismatch(es) during replay").into());
    }
    Ok(())
}

pub fn clean() -> RunResult<()> {
    if Path::new("tmp").exists() {
        std::fs::remove_dir_all("tmp")?;
        println!("removed tmp/");
    } else {
        println!("nothing to clean");
    }
    Ok(())
}
