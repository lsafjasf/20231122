//! 审计流水线：把快照、解析、GStreamer 核验、合并判定串成可回溯的处理链。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use glib::Checksum;

use crate::gst_check;
use crate::merger;
use crate::model::{GstCheck, MergeFinding, Snapshot};
use crate::parser;
use crate::sample;
use crate::workspace::TempWorkspace;

/// 一条处理步骤日志（谁、何时、做了什么、结果如何）。
#[derive(Debug, Clone)]
pub struct ProcessingStep {
    pub seq: usize,
    pub stage: String,
    pub target: String,
    pub detail: String,
    pub ok: bool,
}

/// 一次完整来源检查的结果。
#[derive(Debug)]
pub struct AuditReport {
    pub started_at: String,
    pub steps: Vec<ProcessingStep>,
    pub snapshot_a: Snapshot,
    pub snapshot_b: Snapshot,
    pub gst_a: GstCheck,
    pub gst_b: GstCheck,
    pub findings: Vec<MergeFinding>,
    pub report_path: PathBuf,
    pub temp_root_created: PathBuf,
    pub temp_root_existed_before_cleanup: bool,
    pub temp_root_exists_after_cleanup: bool,
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut checksum =
        Checksum::new(glib::ChecksumType::Sha256).expect("SHA-256 受 GLib 支持，构造不会失败");
    checksum.update(bytes);
    checksum.string().expect("SHA-256 十六进制结果可用")
}

fn now_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

struct StepLogger {
    steps: Vec<ProcessingStep>,
}

impl StepLogger {
    fn new() -> Self {
        Self { steps: Vec::new() }
    }

    fn log(&mut self, stage: &str, target: &str, detail: impl Into<String>, ok: bool) {
        let seq = self.steps.len() + 1;
        self.steps.push(ProcessingStep {
            seq,
            stage: stage.to_string(),
            target: target.to_string(),
            detail: detail.into(),
            ok,
        });
    }
}

fn build_snapshot(
    logger: &mut StepLogger,
    id: &str,
    label: &str,
    lang: &str,
    raw: Vec<u8>,
    ws: &TempWorkspace,
) -> Snapshot {
    let sha = sha256_hex(&raw);
    let line_count = raw.iter().filter(|b| **b == b'\n').count() + 1;
    let file_name = format!("{id}.srt");

    let staged_path = match ws.stage(&file_name, &raw) {
        Ok(path) => {
            logger.log(
                "落盘快照",
                id,
                format!("{} 字节写入临时工作区 {:?}", raw.len(), path),
                true,
            );
            path
        }
        Err(err) => {
            logger.log("落盘快照", id, format!("写入失败: {err}"), false);
            PathBuf::from("<unstaged>")
        }
    };

    // 单快照解析异常隔离：即便内部出现 panic，也不影响另一份快照。
    let parse_result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parser::parse_srt(id, &raw)));
    let cues = match parse_result {
        Ok(cues) => {
            logger.log(
                "SRT 解析",
                id,
                format!("解析出 {} 条记录", cues.len()),
                true,
            );
            cues
        }
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "未知 panic".to_string());
            logger.log("SRT 解析", id, format!("解析 panic，已隔离: {msg}"), false);
            Vec::new()
        }
    };

    let defect_total: usize = cues.iter().map(|c| c.defects.len()).sum();
    logger.log(
        "缺陷扫描",
        id,
        format!("累计缺陷 {defect_total} 项，原始字节与行号已随记录保留"),
        true,
    );

    Snapshot {
        id: id.to_string(),
        label: label.to_string(),
        lang: lang.to_string(),
        raw_bytes: raw,
        sha256: sha,
        byte_len: 0,
        line_count,
        staged_path,
        cues,
    }
    .with_byte_len()
}

impl Snapshot {
    fn with_byte_len(mut self) -> Self {
        self.byte_len = self.raw_bytes.len();
        self
    }
}

fn run_gst_isolated(logger: &mut StepLogger, id: &str, path: &Path) -> GstCheck {
    if !path.exists() {
        let check = GstCheck {
            snapshot_id: id.to_string(),
            error: Some("快照未落盘，跳过 GStreamer 核验".to_string()),
            ..Default::default()
        };
        logger.log("GStreamer 核验", id, check.error.clone().unwrap(), false);
        return check;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        gst_check::check_snapshot(id, path)
    }));
    match result {
        Ok(check) => {
            logger.log(
                "GStreamer 核验",
                id,
                format!(
                    "pipeline 成功={}，吐出字幕 {} 条，bus={}",
                    check.success, check.emitted_cues, check.bus_message
                ),
                check.success,
            );
            if let Some(err) = &check.error {
                logger.log("GStreamer 异常隔离", id, err.clone(), false);
            }
            check
        }
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "未知 panic".to_string());
            logger.log("GStreamer 核验", id, format!("panic 已隔离: {msg}"), false);
            let check = GstCheck {
                snapshot_id: id.to_string(),
                error: Some(format!("panic 已隔离: {msg}")),
                ..Default::default()
            };
            check
        }
    }
}

/// 执行完整检查流水线，返回报告（报告已落盘，临时工作区已清理）。
pub fn run_pipeline(report_dir: &Path) -> anyhow_lite::Result<AuditReport> {
    std::fs::create_dir_all(report_dir)?;
    let started_at = now_stamp();
    let mut logger = StepLogger::new();

    let mut ws = TempWorkspace::create()?;
    let temp_root_created = ws.root().to_path_buf();
    logger.log(
        "建立临时工作区",
        "workspace",
        format!("{:?}", temp_root_created),
        true,
    );

    let raw_a = sample::snapshot_a_bytes();
    let raw_b = sample::snapshot_b_bytes();
    logger.log(
        "载入原始输入",
        "snapshot_a/snapshot_b",
        format!(
            "A={} 字节, B={} 字节（原样保留，不转码）",
            raw_a.len(),
            raw_b.len()
        ),
        true,
    );

    let snapshot_a = build_snapshot(&mut logger, "snapshot_a", "英文版本 V1", "en", raw_a, &ws);
    let snapshot_b = build_snapshot(&mut logger, "snapshot_b", "中文版本 V2", "zh", raw_b, &ws);

    let gst_a = run_gst_isolated(&mut logger, "snapshot_a", &snapshot_a.staged_path);
    // 即使 A 的核验失败，也继续核验 B。
    let gst_b = run_gst_isolated(&mut logger, "snapshot_b", &snapshot_b.staged_path);

    let findings = merger::merge(&snapshot_a, &snapshot_b);
    for finding in &findings {
        logger.log(
            "合并判定",
            &format!("finding#{}", finding.index),
            format!(
                "结论={}，证据 {} 条，A 记录={:?}，B 记录={:?}",
                finding.verdict.as_str(),
                finding.evidence.len(),
                finding.a_records,
                finding.b_records
            ),
            finding.verdict != crate::model::Verdict::Malformed,
        );
    }

    let temp_root_existed_before_cleanup = ws.exists();
    ws.cleanup()?;
    let temp_root_exists_after_cleanup = ws.exists();
    logger.log(
        "清理临时状态",
        "workspace",
        format!(
            "清理前存在={temp_root_existed_before_cleanup}，清理后存在={temp_root_exists_after_cleanup}"
        ),
        !temp_root_exists_after_cleanup,
    );

    let report_path = report_dir.join("source-check-report.txt");
    let report = AuditReport {
        started_at,
        steps: logger.steps,
        snapshot_a,
        snapshot_b,
        gst_a,
        gst_b,
        findings,
        report_path: report_path.clone(),
        temp_root_created,
        temp_root_existed_before_cleanup,
        temp_root_exists_after_cleanup,
    };
    write_report(&report, &report_path)?;
    Ok(report)
}

fn write_report(report: &AuditReport, path: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    writeln!(file, "多语言字幕时间轴合并 — 来源检查报告")?;
    writeln!(file, "生成时间: {}", report.started_at)?;
    writeln!(file)?;

    for snapshot in [&report.snapshot_a, &report.snapshot_b] {
        writeln!(
            file,
            "== {} ({}, {}) ==",
            snapshot.id, snapshot.label, snapshot.lang
        )?;
        writeln!(file, "SHA-256 : {}", snapshot.sha256)?;
        writeln!(
            file,
            "字节数  : {}，行数: {}，临时落盘: {:?}",
            snapshot.byte_len, snapshot.line_count, snapshot.staged_path
        )?;
        writeln!(file, "记录数  : {}", snapshot.cues.len())?;
        for cue in &snapshot.cues {
            writeln!(
                file,
                "  #{} 行{}-{} seq={:?} 缺陷={}",
                cue.loc.record_index,
                cue.loc.line_start,
                cue.loc.line_end,
                cue.seq,
                if cue.defects.is_empty() {
                    "无".to_string()
                } else {
                    cue.defects.join("；")
                }
            )?;
        }
        writeln!(file)?;
    }

    for check in [&report.gst_a, &report.gst_b] {
        writeln!(file, "== GStreamer 核验 {} ==", check.snapshot_id)?;
        writeln!(file, "pipeline : {}", check.pipeline_desc)?;
        writeln!(
            file,
            "成功={}，字幕条数={}，首帧={:?}，末帧结束={:?}",
            check.success, check.emitted_cues, check.first_start_ns, check.last_end_ns
        )?;
        writeln!(file, "bus      : {}", check.bus_message)?;
        if let Some(err) = &check.error {
            writeln!(file, "错误(已隔离): {err}")?;
        }
        writeln!(file)?;
    }

    writeln!(file, "== 合并结论（每条均可回溯到上述原始记录） ==")?;
    for finding in &report.findings {
        writeln!(
            file,
            "[#{}] {} — {}",
            finding.index,
            finding.verdict.as_str(),
            finding.reason
        )?;
        for evidence in &finding.evidence {
            writeln!(file, "    {evidence}")?;
        }
    }
    writeln!(file)?;

    writeln!(file, "== 处理步骤 ==")?;
    for step in &report.steps {
        writeln!(
            file,
            "{:>2}. [{}] {} / {} — {}",
            step.seq,
            if step.ok { "OK" } else { "FAIL" },
            step.stage,
            step.target,
            step.detail
        )?;
    }
    writeln!(file)?;
    writeln!(
        file,
        "临时工作区 {:?}：清理前存在={}，清理后存在={}",
        report.temp_root_created,
        report.temp_root_existed_before_cleanup,
        report.temp_root_exists_after_cleanup
    )?;
    Ok(())
}

/// 极简内部错误类型，避免引入额外错误处理 crate。
pub mod anyhow_lite {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pipeline_covers_all_required_outcomes() {
        let dir = std::env::temp_dir().join(format!(
            "submerge-check-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let report = run_pipeline(&dir).expect("流水线应跑通");

        use crate::model::Verdict;
        let verdicts: Vec<&Verdict> = report.findings.iter().map(|f| &f.verdict).collect();
        assert!(
            verdicts.contains(&&Verdict::Ambiguous),
            "必须包含无法判断边界"
        );
        assert!(verdicts.contains(&&Verdict::Truncated), "必须包含截断记录");
        assert!(verdicts.contains(&&Verdict::Malformed), "必须包含损坏记录");
        assert!(verdicts.contains(&&Verdict::Aligned), "必须包含已对齐结论");
        assert!(
            verdicts.contains(&&Verdict::Overlap),
            "必须包含部分重叠结论"
        );

        // 每个结论都带证据与原始记录指针。
        for finding in &report.findings {
            assert!(!finding.evidence.is_empty());
            assert!(!finding.b_records.is_empty());
        }

        // 两份快照都有哈希，且互不相同。
        assert_eq!(report.snapshot_a.sha256.len(), 64);
        assert_ne!(report.snapshot_a.sha256, report.snapshot_b.sha256);

        // GStreamer 双快照核验成功。
        assert!(report.gst_a.success);
        assert!(report.gst_b.success);

        // 临时状态必须被清理，持久报告必须存在。
        assert!(!report.temp_root_exists_after_cleanup);
        assert!(report.report_path.exists());

        // 审计步骤覆盖完整处理链。
        let stages: Vec<&str> = report.steps.iter().map(|s| s.stage.as_str()).collect();
        for required in [
            "建立临时工作区",
            "载入原始输入",
            "SRT 解析",
            "GStreamer 核验",
            "合并判定",
            "清理临时状态",
        ] {
            assert!(stages.contains(&required), "缺少处理阶段: {required}");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gst_failure_is_isolated_and_next_object_continues() {
        let ws = TempWorkspace::create().unwrap();
        let good = ws
            .stage("good.srt", b"1\n00:00:01,000 --> 00:00:02,000\nok\n\n")
            .unwrap();
        let missing = ws.root().join("does-not-exist.srt");
        let mut logger = StepLogger::new();

        // 先处理一个必然失败的对象（文件不存在）。
        let failed = run_gst_isolated(&mut logger, "snapshot_missing", &missing);
        assert!(!failed.success);
        assert!(failed.error.unwrap().contains("跳过"));

        // 异常之后，下一个对象仍被正常处理。
        let ok = run_gst_isolated(&mut logger, "snapshot_good", &good);
        assert!(ok.success, "异常后应继续处理其余对象: {:?}", ok.error);
        assert_eq!(ok.emitted_cues, 1);
        assert!(logger.steps.iter().any(|s| !s.ok), "失败必须留痕");
        assert!(logger.steps.iter().filter(|s| s.ok).count() >= 1);
    }
}
