//! Report export and independent verification.
//!
//! Export writes `report.json` (ordered fields, atomic write) and a
//! human-readable `report.md`. A colleague can later run the `verify`
//! subcommand on the same directory: it replays the evidence hash chain,
//! recomputes every digest, checks the resume-consistency comparison,
//! and confirms that reloading and re-saving the report preserves both
//! field order and content byte-for-byte.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::evidence;
use crate::model::*;

pub fn write_report(dir: &Path, report: &Report) -> Result<()> {
    let json = serde_json::to_string_pretty(report)?;
    let tmp = dir.join("report.json.tmp");
    fs::write(&tmp, &json)?;
    fs::rename(&tmp, dir.join("report.json"))?;
    fs::write(dir.join("report.md"), render_markdown(report))?;
    Ok(())
}

fn render_markdown(r: &Report) -> String {
    let mut md = String::new();
    md.push_str(&format!("# 字幕字体覆盖报告 / Subtitle Font Coverage\n\n"));
    md.push_str(&format!("- 对象 Object: `{}` ({})\n", r.object_id, r.object_path));
    md.push_str(&format!("- SHA-256: `{}`\n", r.object_sha256));
    md.push_str(&format!(
        "- 字幕 Cues: {}/{} processed, {} degraded\n",
        r.processed_cues, r.total_cues, r.degraded_cues
    ));
    md.push_str(&format!(
        "- 时钟校准 Clock offset (analysis - capture): {} ms ({} paired samples, max dev {} ms)\n",
        r.clock_calibration.estimated_offset_ms,
        r.clock_calibration.paired_samples,
        r.clock_calibration.max_abs_deviation_ms
    ));
    if let Some(cmp) = &r.resume_comparison {
        md.push_str(&format!(
            "- 恢复一致性 Resume check: window {} cues, identical = **{}**\n",
            cmp.window_size, cmp.identical
        ));
    }
    md.push_str("\n## 会话 Sessions\n\n");
    for s in &r.sessions {
        md.push_str(&format!(
            "- `{}`: cues {}..{} ({})\n",
            s.session_id, s.cues_from, s.cues_to, s.ended_by
        ));
    }
    md.push_str("\n## 降级事件 Degradation events\n\n");
    if r.degradation_events.is_empty() {
        md.push_str("- none\n");
    }
    for d in &r.degradation_events {
        md.push_str(&format!("- cue {}: {}\n", d.at_cue_index, d.reason));
    }
    md.push_str("\n## 字形覆盖 Glyph coverage\n\n");
    md.push_str("| char | codepoint | status | font | occurrences |\n|---|---|---|---|---|\n");
    for e in &r.coverage {
        md.push_str(&format!(
            "| {} | {} | {:?} | {} | {} |\n",
            e.char,
            e.codepoint,
            e.status,
            e.font.clone().unwrap_or_default(),
            e.occurrences
        ));
    }
    md.push_str(&format!("\n证据摘要 Evidence SHA-256: `{}`\n", r.evidence_sha256));
    md.push_str(&format!("报告摘要 Report SHA-256: `{}`\n", r.report_sha256));
    md
}

#[derive(Debug)]
pub struct VerifyOutcome {
    pub checks: Vec<String>,
}

/// Independently verify an exported directory. Returns the list of
/// checks that passed; any failure aborts with an error.
pub fn verify(dir: &Path) -> Result<VerifyOutcome> {
    let mut checks = Vec::new();

    // 1. Reload report and confirm byte-stable round trip (field order
    //    and content preserved).
    let report_path = dir.join("report.json");
    let raw = fs::read_to_string(&report_path)
        .with_context(|| format!("missing {}", report_path.display()))?;
    let report: Report = serde_json::from_str(&raw).context("report.json is not valid")?;
    let reserialized = serde_json::to_string_pretty(&report)?;
    anyhow::ensure!(
        reserialized == raw,
        "report round trip changed bytes: field order or content not preserved"
    );
    checks.push("report reload preserves fields and order (byte-stable)".to_string());

    // 2. Report self-hash.
    anyhow::ensure!(
        report.compute_hash() == report.report_sha256,
        "report_sha256 mismatch"
    );
    checks.push("report_sha256 recomputed OK".to_string());

    // 3. Evidence chain integrity and digest.
    let evidence_path = dir.join("evidence.jsonl");
    let events = evidence::verify_chain(&evidence_path)?;
    let digest = sha256_hex(&fs::read(&evidence_path)?);
    anyhow::ensure!(
        digest == report.evidence_sha256,
        "evidence digest does not match report"
    );
    checks.push(format!("evidence chain verified ({events} events, digest matches)"));

    // 4. Resume-consistency comparison, if a pause/resume happened.
    if let Some(cmp) = &report.resume_comparison {
        anyhow::ensure!(
            cmp.identical,
            "resume verification window differs before/after resume"
        );
        checks.push(format!(
            "resume comparison identical across {} cues",
            cmp.window_size
        ));
    }

    // 5. Checkpoint integrity, if still present.
    let ckpt_path = dir.join("checkpoint.json");
    if ckpt_path.exists() {
        let ckpt: Checkpoint = serde_json::from_str(&fs::read_to_string(&ckpt_path)?)?;
        anyhow::ensure!(
            ckpt.compute_hash() == ckpt.checkpoint_sha256,
            "checkpoint hash mismatch"
        );
        checks.push("checkpoint hash OK".to_string());
    }

    // 6. Media object hash, if the object is still reachable.
    let object = Path::new(&report.object_path);
    if object.exists() {
        let bytes = fs::read(object)?;
        anyhow::ensure!(
            sha256_hex(&bytes) == report.object_sha256,
            "media object hash mismatch"
        );
        checks.push("media object sha256 matches report".to_string());
    }

    Ok(VerifyOutcome { checks })
}
