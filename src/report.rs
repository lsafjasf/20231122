use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::failure::{Failure, Result};
use crate::manifest::{CaseManifest, LossEvent, format_parameters};
use crate::waveform::{IntervalMetrics, WaveformSummary, amplitude_db};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Pass,
    Attention,
    Fail,
    Inconclusive,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pass => "通过",
            Self::Attention => "需关注",
            Self::Fail => "失败",
            Self::Inconclusive => "无法定论",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomatedCheck {
    pub id: String,
    pub title: String,
    pub result: Verdict,
    pub detail: String,
    pub action: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventConclusion {
    pub event_id: String,
    pub start_ms: f64,
    pub end_ms: f64,
    pub kind: String,
    pub track_id: Option<String>,
    pub track_index: Option<u64>,
    pub frame: Option<u64>,
    pub processing_id: Option<String>,
    pub parameters: String,
    pub result: Verdict,
    pub metrics: Option<IntervalMetrics>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomatedReport {
    pub schema_version: u32,
    pub generated_at: String,
    pub case_id: String,
    pub verdict: Verdict,
    pub summary: String,
    pub checks: Vec<AutomatedCheck>,
    pub events: Vec<EventConclusion>,
    pub limitations: Vec<String>,
    pub unknown_extension: Option<String>,
    pub evidence: EvidenceIndex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceFile {
    pub name: String,
    pub path: String,
    pub sha256: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EvidenceIndex {
    pub root: PathBuf,
    pub files: Vec<EvidenceFile>,
    pub original_audio: Option<EvidenceFile>,
    pub unknown_extension: Option<String>,
}

impl AutomatedReport {
    pub fn from_failure(
        case_id: &str,
        failure: &Failure,
        evidence: EvidenceIndex,
    ) -> AutomatedReport {
        Self {
            schema_version: 1,
            generated_at: chrono::Utc::now().to_rfc3339(),
            case_id: case_id.to_string(),
            verdict: Verdict::Fail,
            summary: format!("评估未能完成：{}", failure.message),
            checks: vec![AutomatedCheck {
                id: failure.code.clone(),
                title: "评估执行".to_string(),
                result: Verdict::Fail,
                detail: failure.to_string(),
                action: Some(failure.action.clone()),
            }],
            events: Vec::new(),
            limitations: vec!["失败报告不包含可听辨结论，也不代表掩蔽质量通过或失败。".to_string()],
            unknown_extension: evidence.unknown_extension.clone(),
            evidence,
        }
    }

    pub fn overall_verdict(checks: &[AutomatedCheck]) -> Verdict {
        if checks.iter().any(|check| check.result == Verdict::Fail) {
            Verdict::Fail
        } else if checks
            .iter()
            .any(|check| check.result == Verdict::Attention)
        {
            Verdict::Attention
        } else if checks
            .iter()
            .any(|check| check.result == Verdict::Inconclusive)
        {
            Verdict::Inconclusive
        } else {
            Verdict::Pass
        }
    }

    pub fn next_actions(&self) -> Vec<String> {
        self.checks
            .iter()
            .filter(|check| check.result != Verdict::Pass)
            .filter_map(|check| check.action.clone())
            .collect()
    }
}

pub fn build_automated(
    manifest: &CaseManifest,
    waveform: &WaveformSummary,
    unknown_extension: Option<String>,
    migrated: bool,
    evidence: EvidenceIndex,
) -> AutomatedReport {
    let mut checks = Vec::new();
    checks.push(AutomatedCheck {
        id: "decode".to_string(),
        title: "输出波形可解码".to_string(),
        result: Verdict::Pass,
        detail: format!(
            "GStreamer 已解码 {:.3} 秒音频并生成 48 kHz 单声道输出波形。",
            waveform.duration_ms / 1000.0
        ),
        action: None,
    });
    checks.push(AutomatedCheck {
        id: "unknown_extension".to_string(),
        title: "扩展名记录".to_string(),
        result: if unknown_extension.is_some() {
            Verdict::Attention
        } else {
            Verdict::Pass
        },
        detail: match &unknown_extension {
            Some(extension) => format!(
                "原始文件扩展名为 `{extension}`，不在已知音频扩展名列表；已按文件内容交由 GStreamer 解码并保留原始文件。"
            ),
            None => "输入扩展名在已知音频扩展名列表内；仍以文件内容解码结果为准。".to_string(),
        },
        action: unknown_extension
            .as_ref()
            .map(|_| "人工确认该扩展名对应的采集/恢复流程，并检查证据包中的原始文件哈希。".to_string()),
    });
    checks.push(AutomatedCheck {
        id: "schema".to_string(),
        title: "清单格式兼容性".to_string(),
        result: if migrated {
            Verdict::Attention
        } else {
            Verdict::Pass
        },
        detail: format!(
            "识别为 {} 格式（{}），归一化为 v{}。",
            manifest.schema.detected_format,
            manifest.schema.input_version,
            manifest.schema.normalized_version
        ),
        action: migrated.then(|| {
            "旧格式已迁移；请人工抽查时间点、轨道号和参数是否与原始工单一致。".to_string()
        }),
    });
    checks.push(AutomatedCheck {
        id: "events_present".to_string(),
        title: "事件可定位".to_string(),
        result: if manifest.events.is_empty() {
            Verdict::Attention
        } else {
            Verdict::Pass
        },
        detail: if manifest.events.is_empty() {
            "清单未提供丢包/掩蔽事件，自动流程无法声称通过。".to_string()
        } else {
            format!("清单提供 {} 个可定位事件。", manifest.events.len())
        },
        action: manifest
            .events
            .is_empty()
            .then(|| "补充 `events`，至少包含时间点；再重新生成证据包。".to_string()),
    });
    checks.push(AutomatedCheck {
        id: "duration".to_string(),
        title: "波形完整性".to_string(),
        result: if waveform.truncated {
            Verdict::Attention
        } else if waveform.peak_dbfs <= -80.0 {
            Verdict::Fail
        } else {
            Verdict::Pass
        },
        detail: format!(
            "时长 {:.3} 秒，峰值 {:.1} dBFS，RMS {:.1} dBFS，削波占比 {:.4}%{}。",
            waveform.duration_ms / 1000.0,
            waveform.peak_dbfs,
            waveform.rms_dbfs,
            waveform.clipped_fraction * 100.0,
            if waveform.truncated {
                "；受分析上限截断"
            } else {
                ""
            }
        ),
        action: if waveform.truncated {
            Some("设置更大的 `ALC_MAX_SECONDS` 或截取相关重放片段后重新评估。".to_string())
        } else if waveform.peak_dbfs <= -80.0 {
            Some("检查重放音量、路由和采集增益；当前输出几乎无声，不能确认掩蔽有效。".to_string())
        } else {
            None
        },
    });

    let mut event_conclusions = Vec::new();
    for event in &manifest.events {
        event_conclusions.push(evaluate_event(event, waveform, manifest));
    }
    let event_failures = event_conclusions
        .iter()
        .filter(|event| event.result == Verdict::Fail)
        .count();
    let event_attention = event_conclusions
        .iter()
        .filter(|event| event.result == Verdict::Attention)
        .count();
    checks.push(AutomatedCheck {
        id: "event_windows".to_string(),
        title: "事件窗口幅度检查".to_string(),
        result: if event_failures > 0 {
            Verdict::Fail
        } else if event_attention > 0 || waveform.clipped_fraction > 0.001 {
            Verdict::Attention
        } else {
            Verdict::Pass
        },
        detail: format!(
            "{} 个事件窗口中 {} 个材料不足，{} 个需关注；全局削波占比 {:.4}%。",
            event_conclusions.len(),
            event_failures,
            event_attention,
            waveform.clipped_fraction * 100.0
        ),
        action: (event_failures > 0 || event_attention > 0).then(|| {
            "逐个点击事件时间点回放，并对照原始清单复查轨道、帧号和掩蔽参数。".to_string()
        }),
    });

    let verdict = AutomatedReport::overall_verdict(&checks);
    let summary = match verdict {
        Verdict::Pass => "自动检查未发现解码、映射或幅度异常；仍需人工听辨确认。".to_string(),
        Verdict::Attention => "自动检查发现需人工解释的事项，不能直接判定为通过。".to_string(),
        Verdict::Fail => "自动检查发现失败项；请先执行整改建议，再重新生成证据包。".to_string(),
        Verdict::Inconclusive => "自动检查材料不足，无法形成结论。".to_string(),
    };
    AutomatedReport {
        schema_version: 1,
        generated_at: chrono::Utc::now().to_rfc3339(),
        case_id: manifest.case.case_id.clone(),
        verdict,
        summary,
        checks,
        events: event_conclusions,
        limitations: vec![
            "自动结论基于时间窗幅度、静音、削波与映射检查，不评价听感自然度。".to_string(),
            "事件参数来自清单；证据包不会猜测未记录的处理算法或编解码参数。".to_string(),
            "输出波形为 48 kHz 单声道归一化分析副本，原始重放文件已单独保留。".to_string(),
        ],
        unknown_extension,
        evidence,
    }
}

fn evaluate_event(
    event: &LossEvent,
    waveform: &WaveformSummary,
    manifest: &CaseManifest,
) -> EventConclusion {
    let end_ms = event.end_or_start_ms();
    let metrics = waveform.metrics_for(event.start_ms, end_ms);
    let mut notes = Vec::new();
    let mut result = Verdict::Pass;

    if event.start_ms > waveform.duration_ms
        || end_ms > waveform.duration_ms + 0.5
        || event.duration_ms == Some(0.0)
    {
        result = Verdict::Fail;
        notes.push("事件窗口超出输出波形范围或时长为零。".to_string());
    }
    if manifest
        .tracks
        .iter()
        .all(|track| Some(&track.id) != event.track_id.as_ref())
    {
        if result != Verdict::Fail {
            result = Verdict::Attention;
        }
        notes.push("轨道未在清单中解析到。".to_string());
    }
    if event.frame.is_none() {
        if result != Verdict::Fail {
            result = Verdict::Attention;
        }
        notes.push("未记录帧号。".to_string());
    }
    if event.effective_parameters.is_empty() {
        if result != Verdict::Fail {
            result = Verdict::Attention;
        }
        notes.push("未记录处理参数。".to_string());
    }
    if let Some(metrics) = &metrics {
        if metrics.silent_fraction >= 0.9 {
            if result != Verdict::Fail {
                result = Verdict::Attention;
            }
            notes.push(format!(
                "窗口内 {:.1}% 时长接近静音，掩蔽可能缺失。",
                metrics.silent_fraction * 100.0
            ));
        }
        if metrics.clipped_fraction > 0.0 {
            if result != Verdict::Fail {
                result = Verdict::Attention;
            }
            notes.push("窗口内检测到削波。".to_string());
        }
        if metrics.delta_db.is_some_and(|delta| delta.abs() > 18.0) {
            if result != Verdict::Fail {
                result = Verdict::Attention;
            }
            notes.push("窗口 RMS 与前置上下文差异超过 18 dB。".to_string());
        }
        if metrics.max_step_db > 20.0 {
            if result != Verdict::Fail {
                result = Verdict::Attention;
            }
            notes.push("窗口内相邻 10 ms 幅度跃变超过 20 dB。".to_string());
        }
    } else {
        result = Verdict::Fail;
        notes.push("输出波形中没有对应事件窗口。".to_string());
    }

    EventConclusion {
        event_id: event.id.clone(),
        start_ms: event.start_ms,
        end_ms,
        kind: event.kind.clone(),
        track_id: event.track_id.clone(),
        track_index: event.track_index,
        frame: event.frame,
        processing_id: event.processing_id.clone(),
        parameters: format_parameters(&event.effective_parameters),
        result,
        metrics,
        notes,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualVerdict {
    Pending,
    Pass,
    Attention,
    Fail,
}

impl ManualVerdict {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "待确认",
            Self::Pass => "通过",
            Self::Attention => "需关注",
            Self::Fail => "不通过",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualItem {
    pub id: String,
    pub label: String,
    pub verdict: ManualVerdict,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualConfirmation {
    pub schema_version: u32,
    pub case_id: String,
    pub status: ManualVerdict,
    pub reviewer: String,
    pub reviewed_at: Option<String>,
    pub items: Vec<ManualItem>,
    pub notes: String,
}

impl ManualConfirmation {
    pub fn initial(case_id: &str, automated: &AutomatedReport, extension: Option<&str>) -> Self {
        let mut items = vec![
            ManualItem {
                id: "evidence.original-audio".to_string(),
                label: "原始重放文件与证据包中的 SHA-256 一致".to_string(),
                verdict: ManualVerdict::Pending,
                note: String::new(),
            },
            ManualItem {
                id: "replay.full-listen".to_string(),
                label: "已完整回放一次，未发现未记录的断音、爆音或节奏漂移".to_string(),
                verdict: ManualVerdict::Pending,
                note: String::new(),
            },
            ManualItem {
                id: "replay.cross-check".to_string(),
                label: "时间点、轨道/帧与处理参数已和原始恢复记录交叉核对".to_string(),
                verdict: ManualVerdict::Pending,
                note: String::new(),
            },
        ];
        if let Some(extension) = extension {
            items.push(ManualItem {
                id: "evidence.unknown-extension".to_string(),
                label: format!("已确认未知扩展名 `{extension}` 对应的恢复/采集流程"),
                verdict: ManualVerdict::Pending,
                note: String::new(),
            });
        }
        for event in &automated.events {
            items.push(ManualItem {
                id: format!("event.{}", event.event_id),
                label: format!(
                    "听辨事件 {} @ {:.3}s（轨道 {}，帧 {}）",
                    event.event_id,
                    event.start_ms / 1000.0,
                    event.track_id.as_deref().unwrap_or("未知"),
                    event
                        .frame
                        .map(|frame| frame.to_string())
                        .unwrap_or_else(|| "未知".to_string())
                ),
                verdict: ManualVerdict::Pending,
                note: String::new(),
            });
        }
        let mut confirmation = Self {
            schema_version: 1,
            case_id: case_id.to_string(),
            status: ManualVerdict::Pending,
            reviewer: String::new(),
            reviewed_at: None,
            items,
            notes: String::new(),
        };
        confirmation.recompute_status();
        confirmation
    }

    pub fn recompute_status(&mut self) {
        if self
            .items
            .iter()
            .any(|item| item.verdict == ManualVerdict::Fail)
        {
            self.status = ManualVerdict::Fail;
        } else if self
            .items
            .iter()
            .any(|item| item.verdict == ManualVerdict::Attention)
        {
            self.status = ManualVerdict::Attention;
        } else if self
            .items
            .iter()
            .any(|item| item.verdict == ManualVerdict::Pending)
        {
            self.status = ManualVerdict::Pending;
        } else {
            self.status = ManualVerdict::Pass;
        }
    }

    pub fn merge_recovered(&mut self, existing: &ManualConfirmation) {
        if !existing.reviewer.is_empty() {
            self.reviewer = existing.reviewer.clone();
        }
        self.reviewed_at = existing.reviewed_at.clone();
        self.notes = existing.notes.clone();
        for item in &mut self.items {
            if let Some(previous) = existing.items.iter().find(|old| old.id == item.id) {
                item.verdict = previous.verdict;
                item.note = previous.note.clone();
            }
        }
        self.recompute_status();
    }
}

pub fn load_manifest_file(path: &Path) -> Option<serde_json::Value> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn load_manual(path: &Path) -> Option<ManualConfirmation> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn write_reports(
    directory: &Path,
    manifest: &CaseManifest,
    automated: &AutomatedReport,
    manual: &ManualConfirmation,
) -> Result<()> {
    fs::create_dir_all(directory).map_err(|e| Failure::io("创建报告目录", directory, e))?;
    let automated_path = directory.join("automated-conclusions.json");
    let manual_path = directory.join("manual-confirmation.json");
    let report_json_path = directory.join("report.json");
    let report_md_path = directory.join("report.md");
    write_json(&automated_path, automated)?;
    write_json(&manual_path, manual)?;
    let report = serde_json::json!({
        "schema_version": 1,
        "automated": automated,
        "manual": manual,
    });
    write_json(&report_json_path, &report)?;
    let markdown = render_markdown(manifest, automated, manual);
    fs::write(&report_md_path, markdown)
        .map_err(|e| Failure::io("写入 Markdown 报告", &report_md_path, e))?;
    Ok(())
}

pub fn update_manual_report(
    directory: &Path,
    manifest: &CaseManifest,
    automated: &AutomatedReport,
    manual: &ManualConfirmation,
) -> Result<()> {
    let manual_path = directory.join("manual-confirmation.json");
    let report_json_path = directory.join("report.json");
    let report_md_path = directory.join("report.md");
    write_json(&manual_path, manual)?;
    write_json(
        &report_json_path,
        &serde_json::json!({
            "schema_version": 1,
            "automated": automated,
            "manual": manual,
        }),
    )?;
    fs::write(
        &report_md_path,
        render_markdown(manifest, automated, manual),
    )
    .map_err(|e| Failure::io("更新 Markdown 报告", &report_md_path, e))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        Failure::new(
            "report_json",
            format!("序列化报告失败：{}", path.display()),
            "保留现有证据并重新运行评估；这通常表示内部数据异常。",
        )
        .with_detail(error.to_string())
    })?;
    fs::write(path, bytes).map_err(|e| Failure::io("写入 JSON 报告", path, e))
}

pub fn render_markdown(
    manifest: &CaseManifest,
    automated: &AutomatedReport,
    manual: &ManualConfirmation,
) -> String {
    let mut output = String::new();
    output.push_str("# 音频丢包掩蔽评估报告\n\n");
    output.push_str(&format!("- 案件：`{}`\n", manifest.case.case_id));
    output.push_str(&format!(
        "- 恢复时间：{}\n",
        manifest.case.recovered_at.as_deref().unwrap_or("未记录")
    ));
    output.push_str(&format!("- 时间原点：{}\n", manifest.case.replay_origin));
    output.push_str(&format!(
        "- 清单：{} / {}\n",
        manifest.schema.detected_format, manifest.schema.input_version
    ));
    output.push_str(&format!(
        "- 未知扩展：{}\n",
        automated.unknown_extension.as_deref().unwrap_or("无")
    ));
    output.push_str(&format!("- 报告生成：{}\n\n", automated.generated_at));

    output.push_str("## 自动结论\n\n");
    output.push_str(&format!(
        "**{}**：{}\n\n",
        automated.verdict.label(),
        automated.summary
    ));
    output.push_str("| 检查项 | 结果 | 证据 | 后续操作 |\n");
    output.push_str("|---|---|---|---|\n");
    for check in &automated.checks {
        output.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            table(&check.title),
            check.result.label(),
            table(&check.detail),
            table(check.action.as_deref().unwrap_or("—"))
        ));
    }
    output.push('\n');

    if !automated.events.is_empty() {
        output.push_str("### 时间点、轨道/帧、参数与波形\n\n");
        output.push_str("| 事件 | 时间 | 轨道/帧 | 处理参数 | 波形指标 | 自动结果 | 备注 |\n");
        output.push_str("|---|---|---|---|---|---|---|\n");
        for event in &automated.events {
            let metrics = event
                .metrics
                .as_ref()
                .map(|metrics| {
                    format!(
                        "peak {:.1} dBFS / RMS {:.1} dBFS / silent {:.1}% / Δ {:.1} dB",
                        metrics.peak_dbfs,
                        metrics.rms_dbfs,
                        metrics.silent_fraction * 100.0,
                        metrics.delta_db.unwrap_or(f32::NAN)
                    )
                })
                .unwrap_or_else(|| "无窗口".to_string());
            output.push_str(&format!(
                "| `{}` | {:.3}s–{:.3}s | {}/{} | `{}` | {} | {} | {} |\n",
                event.event_id,
                event.start_ms / 1000.0,
                event.end_ms / 1000.0,
                event.track_id.as_deref().unwrap_or("未知"),
                event
                    .frame
                    .map(|frame| frame.to_string())
                    .unwrap_or_else(|| "未知".to_string()),
                table(&event.parameters),
                table(&metrics),
                event.result.label(),
                table(&event.notes.join("；"))
            ));
        }
        output.push('\n');
    }

    let mut actions = automated.next_actions();
    actions.extend(
        automated
            .events
            .iter()
            .filter(|event| event.result != Verdict::Pass)
            .map(|event| {
                format!(
                    "回放 {:.3}s 的事件 {}，核查轨道 {}、帧 {} 与参数 {}。",
                    event.start_ms / 1000.0,
                    event.event_id,
                    event.track_id.as_deref().unwrap_or("未知"),
                    event
                        .frame
                        .map(|frame| frame.to_string())
                        .unwrap_or_else(|| "未知".to_string()),
                    event.parameters
                )
            }),
    );
    if !actions.is_empty() {
        output.push_str("### 自动结论的后续操作\n\n");
        for action in actions {
            output.push_str(&format!("- {action}\n"));
        }
        output.push('\n');
    }

    output.push_str("## 人工确认\n\n");
    output.push_str(&format!(
        "**状态：{}**  \n复核人：{}  \n复核时间：{}\n\n",
        manual.status.label(),
        if manual.reviewer.is_empty() {
            "未填写"
        } else {
            &manual.reviewer
        },
        manual.reviewed_at.as_deref().unwrap_or("未记录")
    ));
    output.push_str("> 人工确认独立于自动结论；下方勾选状态由复核人填写，不由自动检查代填。\n\n");
    for item in &manual.items {
        let marker = match item.verdict {
            ManualVerdict::Pass => "x",
            _ => " ",
        };
        output.push_str(&format!(
            "- [{}] **{}**：{}",
            marker,
            item.verdict.label(),
            item.label
        ));
        if !item.note.is_empty() {
            output.push_str(&format!("（{}）", item.note));
        }
        output.push('\n');
    }
    if !manual.notes.is_empty() {
        output.push_str(&format!("\n人工备注：{}\n", manual.notes));
    }
    output.push('\n');

    output.push_str("## 证据索引\n\n");
    output.push_str(&format!(
        "证据根目录：`{}`\n\n",
        automated.evidence.root.display()
    ));
    output.push_str("| 文件 | SHA-256 | 说明 |\n");
    output.push_str("|---|---|---|\n");
    for file in &automated.evidence.files {
        output.push_str(&format!(
            "| `{}` | `{}` | {} |\n",
            file.path,
            file.sha256.as_deref().unwrap_or("—"),
            table(&file.description)
        ));
    }
    output.push('\n');

    output.push_str("## 局限\n\n");
    for limitation in &automated.limitations {
        output.push_str(&format!("- {limitation}\n"));
    }
    if !manifest.warnings.is_empty() {
        output.push_str("\n清单迁移警告：\n\n");
        for warning in &manifest.warnings {
            output.push_str(&format!("- {warning}\n"));
        }
    }
    output
}

fn table(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', "<br>")
}

pub fn event_wave_db(metrics: &IntervalMetrics) -> String {
    format!(
        "peak {:.1} dBFS / RMS {:.1} dBFS / Δpre {:.1} dB",
        metrics.peak_dbfs,
        metrics.rms_dbfs,
        metrics.delta_db.unwrap_or_else(|| amplitude_db(0.0))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::SchemaInfo;
    use std::collections::BTreeMap;

    #[test]
    fn manual_status_requires_all_items() {
        let mut manual = ManualConfirmation {
            schema_version: 1,
            case_id: "case".to_string(),
            status: ManualVerdict::Pending,
            reviewer: "reviewer".to_string(),
            reviewed_at: None,
            items: vec![
                ManualItem {
                    id: "a".to_string(),
                    label: "a".to_string(),
                    verdict: ManualVerdict::Pass,
                    note: String::new(),
                },
                ManualItem {
                    id: "b".to_string(),
                    label: "b".to_string(),
                    verdict: ManualVerdict::Pending,
                    note: String::new(),
                },
            ],
            notes: String::new(),
        };
        manual.recompute_status();
        assert_eq!(manual.status, ManualVerdict::Pending);
        manual.items[1].verdict = ManualVerdict::Pass;
        manual.recompute_status();
        assert_eq!(manual.status, ManualVerdict::Pass);
    }

    #[test]
    fn markdown_has_separate_sections() {
        let manifest = CaseManifest {
            schema: SchemaInfo {
                detected_format: "new".to_string(),
                input_version: "v2".to_string(),
                normalized_version: 3,
                migrated: false,
                manifest_sha256: "abc".to_string(),
            },
            case: crate::manifest::CaseInfo {
                case_id: "case".to_string(),
                recovered_at: None,
                replay_origin: "0:00".to_string(),
                source_name: None,
                source_note: None,
            },
            audio_path: PathBuf::from("a.wav"),
            tracks: Vec::new(),
            processing: Vec::new(),
            events: Vec::new(),
            warnings: Vec::new(),
        };
        let automated = AutomatedReport {
            schema_version: 1,
            generated_at: "now".to_string(),
            case_id: "case".to_string(),
            verdict: Verdict::Attention,
            summary: "needs review".to_string(),
            checks: Vec::new(),
            events: Vec::new(),
            limitations: vec!["scope".to_string()],
            unknown_extension: Some("rplx".to_string()),
            evidence: EvidenceIndex::default(),
        };
        let manual = ManualConfirmation::initial("case", &automated, Some("rplx"));
        let markdown = render_markdown(&manifest, &automated, &manual);
        assert!(markdown.contains("## 自动结论"));
        assert!(markdown.contains("## 人工确认"));
        assert!(markdown.contains("未知扩展：rplx"));
        let _ = BTreeMap::<String, String>::new();
    }
}
