use crate::experiment::{run_experiment_with_config, ScenarioConfig};
use crate::hash;
use crate::journal::{comparable_record, read_journal};
use std::path::{Path, PathBuf};

pub struct ReplayOutcome {
    pub matched: usize,
    pub total: usize,
    pub mismatches: Vec<String>,
    pub replay_dir: PathBuf,
    pub report_path: PathBuf,
}

pub fn replay_journal(
    journal_path: &Path,
    progress: &mut dyn FnMut(&str),
) -> Result<ReplayOutcome, String> {
    let original = read_journal(journal_path)?;
    if original.is_empty() {
        return Err("日志为空，无法回放".into());
    }
    let config_record = original
        .iter()
        .find(|record| record.step == "experiment-config")
        .ok_or("日志缺少 experiment-config 步骤")?;
    let config: ScenarioConfig = serde_json::from_value(config_record.input.clone())
        .map_err(|e| format!("回放配置无法解析: {e}"))?;
    let replay_dir = journal_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("replay");

    progress("从日志读取配置并重建完整场景");
    let replay_run = run_experiment_with_config(&replay_dir, &config, progress)?;
    let replay_records = read_journal(&replay_run.journal_path)?;
    let total = original.len().max(replay_records.len());
    let mut matched = 0usize;
    let mut mismatches = Vec::new();
    for index in 0..total {
        match (original.get(index), replay_records.get(index)) {
            (Some(want), Some(got)) => {
                let want_value = comparable_record(want);
                let got_value = comparable_record(got);
                if want_value == got_value {
                    matched += 1;
                } else {
                    mismatches.push(format!(
                        "seq {} step {}: original={} replay={}",
                        want.seq,
                        want.step,
                        compact(&want_value),
                        compact(&got_value)
                    ));
                }
            }
            (Some(want), None) => {
                mismatches.push(format!(
                    "seq {} step {} 未在回放中出现",
                    want.seq, want.step
                ));
            }
            (None, Some(got)) => {
                mismatches.push(format!("回放多出 seq {} step {}", got.seq, got.step));
            }
            (None, None) => {}
        }
    }

    let report_path = replay_dir.join("REPLAY.md");
    let report =
        crate::report::render_replay_report(journal_path, &replay_dir, matched, total, &mismatches);
    hash::write_text_atomic(&report_path, &report)?;
    hash::write_json_atomic(
        &replay_dir.join("replay-comparison.json"),
        &serde_json::json!({
            "schema_version": 1,
            "original_journal": journal_path.to_string_lossy(),
            "replay_journal": replay_run.journal_path.to_string_lossy(),
            "matched": matched,
            "total": total,
            "mismatches": mismatches,
        }),
    )?;
    Ok(ReplayOutcome {
        matched,
        total,
        mismatches,
        replay_dir,
        report_path,
    })
}

fn compact(value: &serde_json::Value) -> String {
    let text = serde_json::to_string(value).unwrap_or_else(|_| "<unserializable>".into());
    if text.len() <= 360 {
        text
    } else {
        let mut end = 360;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &text[..end])
    }
}
