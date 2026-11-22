use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::bwf::{default_spec, parse_wave, recover_file, write_fixtures};
use crate::engine::{marker_ids, marker_validation, recovered_control, truncated_probe};
use crate::model::{EvidenceManifest, ExperimentReport, RunKind, StepRecord};
use crate::replay::replay_wav;

#[derive(Debug, Clone)]
pub struct ExperimentOutput {
    pub report: ExperimentReport,
    pub report_path: PathBuf,
    pub evidence_dir: PathBuf,
}

pub fn run_experiment(base_dir: Option<&Path>) -> Result<ExperimentOutput> {
    let workspace = base_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("BWF_REPLAY_ROOT").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")));
    let out_dir = workspace.join("artifacts/bwf-replay-lab");
    let fixture_dir = out_dir.join("fixtures");
    let evidence_dir = out_dir.join("evidence");
    fs::create_dir_all(&fixture_dir)
        .with_context(|| format!("create {}", fixture_dir.display()))?;
    fs::create_dir_all(&evidence_dir)
        .with_context(|| format!("create {}", evidence_dir.display()))?;

    let spec = default_spec();
    let (truncated_path, recovered_path) = write_fixtures(&fixture_dir, &spec)?;
    let truncated = parse_wave(&truncated_path)?;
    let recovered = parse_wave(&recovered_path)?;
    let recovery = recover_file(&truncated_path, &recovered_path)?;

    let probe = truncated_probe(&truncated);
    let expected_frames = recovered.declared_frames();
    let replay = replay_wav(&recovered_path, expected_frames)?;
    fs::write(
        evidence_dir.join("gst-control.json"),
        serde_json::to_vec_pretty(&replay)?,
    )?;

    let ids = marker_ids(&recovered.markers);
    let control = recovered_control(&recovered, &replay, &ids);
    let mut order_ids = ids.clone();
    order_ids.swap(1, 2);
    let order = marker_validation(
        "order_changed",
        "顺序改变：cue-3 先于 cue-2",
        RunKind::OrderChanged,
        &recovered,
        &control,
        &order_ids,
    );
    let duplicate_ids = vec![ids[0], ids[1], ids[1], ids[2], ids[3]];
    let duplicate = marker_validation(
        "duplicate_submission",
        "重复提交：cue-2 提交两次",
        RunKind::DuplicateSubmission,
        &recovered,
        &control,
        &duplicate_ids,
    );

    let runs = vec![probe, control, order, duplicate];
    let first_error = runs.iter().find_map(|run| run.first_error.clone());
    let report_path = evidence_dir.join("report.json");
    let generated_at_utc = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".to_owned());
    let report = ExperimentReport {
        schema_version: 1,
        title: "BWF 时间标记校验可回放实验".to_owned(),
        first_error,
        runs,
        evidence: EvidenceManifest {
            generated_at_utc,
            workspace,
            spec,
            truncated,
            recovered,
            recovery,
            report_path: report_path.clone(),
        },
    };

    write_evidence(&report, &evidence_dir)?;
    Ok(ExperimentOutput {
        report,
        report_path,
        evidence_dir,
    })
}

fn write_evidence(report: &ExperimentReport, evidence_dir: &Path) -> Result<()> {
    let report_path = evidence_dir.join("report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(report)?)
        .with_context(|| format!("write {}", report_path.display()))?;

    let mut timeline = String::new();
    for run in &report.runs {
        for step in &run.steps {
            let line = serde_json::json!({
                "run": run.id,
                "title": run.title,
                "step": step,
            });
            timeline.push_str(&serde_json::to_string(&line)?);
            timeline.push('\n');
        }
    }
    fs::write(evidence_dir.join("steps.jsonl"), timeline)?;

    let first_error = report
        .first_error
        .as_ref()
        .map(serde_json::to_string_pretty)
        .transpose()?
        .unwrap_or_else(|| "null".to_owned());
    fs::write(evidence_dir.join("first-error.json"), first_error)?;
    fs::write(evidence_dir.join("summary.txt"), render_summary(report))?;
    Ok(())
}

fn render_summary(report: &ExperimentReport) -> String {
    let mut out = String::new();
    out.push_str("BWF 时间标记校验 · 可回放实验\n");
    out.push_str("================================\n\n");
    out.push_str(&format!(
        "截断文件: {} 字节 / 声明 {} 字节 data\n",
        report.evidence.truncated.file_len, report.evidence.truncated.data_declared_len
    ));
    out.push_str(&format!(
        "实际 data: {} 字节，恢复补零: {} 字节\n",
        report.evidence.truncated.data_actual_len, report.evidence.recovery.appended_zero_bytes
    ));
    out.push_str(&format!(
        "BEXT 基准: {}\n\n",
        report.evidence.recovered.time_reference_timecode
    ));

    for run in &report.runs {
        let verdict = if run.passed { "PASS" } else { "FAIL" };
        out.push_str(&format!("[{verdict}] {} ({})\n", run.title, run.id));
        for step in &run.steps {
            let marker =
                if run.first_error.as_ref().map(|error| error.step_index) == Some(step.index) {
                    " <-- FIRST ERROR"
                } else {
                    ""
                };
            out.push_str(&format!(
                "  {:02}. {:<22} {:<34} {} -> {}{}\n",
                step.index,
                format!("{:?}", step.phase),
                step.result.code,
                step.state_before.name,
                step.state_after.name,
                marker
            ));
        }
        if let Some(error) = &run.first_error {
            out.push_str(&format!(
                "  首个错误: step {} / {} / expected={} actual={}\n",
                error.step_index, error.code, error.expected, error.actual
            ));
        }
        out.push('\n');
    }

    if let Some(error) = &report.first_error {
        out.push_str(&format!(
            "全局首个错误状态: step {} · {} · {} -> {}\n",
            error.step_index, error.code, error.state_before.name, error.error_state.name
        ));
    }
    out
}

pub fn all_steps(report: &ExperimentReport) -> Vec<(&str, &StepRecord)> {
    report
        .runs
        .iter()
        .flat_map(|run| run.steps.iter().map(move |step| (run.id.as_str(), step)))
        .collect()
}
