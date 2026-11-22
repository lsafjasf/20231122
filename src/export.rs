use anyhow::Result;
use std::fmt::Write as _;
use std::path::Path;

use crate::model::{ConfirmationState, FindingKind, ScanReport, SourceState, StepState};
use crate::util::atomic_write;

pub fn write_markdown(root: &Path, report: &ScanReport) -> Result<String> {
    let mut out = String::new();
    writeln!(out, "# 媒体项目打包清单 · 来源检查报告\n")?;
    writeln!(
        out,
        "> 自动结论与人工确认分节呈现。自动结论只陈述机器可验证事实；人工确认单独记录。\n"
    )?;
    writeln!(out, "- 项目：{}", report.project.package_identity)?;
    writeln!(out, "- 请求：{}", report.project.request)?;
    writeln!(out, "- 审计范围：{}", report.project.audit_scope)?;
    writeln!(out, "- 清单 SHA-256：`{}`", report.project.manifest_sha256)?;
    writeln!(out, "- 运行：`{}`", report.run_id)?;
    writeln!(out, "- 生成：{}\n", report.generated_at)?;

    writeln!(out, "## 自动结论\n")?;
    for finding in &report.auto_conclusions {
        let source = report
            .sources
            .iter()
            .find(|source| source.spec.id == finding.source_id);
        writeln!(
            out,
            "### {} · {} · {}\n",
            finding.id,
            finding_kind_label(finding.kind),
            source
                .map(|item| item.spec.label.as_str())
                .unwrap_or("未知来源")
        )?;
        writeln!(out, "- 结论：{}", finding.headline)?;
        writeln!(out, "- 依据：{}", finding.reasoning)?;
        writeln!(out, "- 策略：{}", finding.policy)?;
        writeln!(
            out,
            "- 步骤：{}",
            finding
                .step_refs
                .iter()
                .map(|seq| format!("S{seq:02}"))
                .collect::<Vec<_>>()
                .join(" / ")
        )?;
        writeln!(out, "- 证据：{}\n", finding.evidence_refs.join(" / "))?;
    }

    writeln!(out, "## 人工确认\n")?;
    for manual in &report.manual_confirmations {
        writeln!(
            out,
            "### {} · {}\n",
            manual_state_label(manual.state),
            manual.title
        )?;
        writeln!(out, "- 问题：{}", manual.question)?;
        writeln!(out, "- 不能自动判断的原因：{}", manual.blocker)?;
        writeln!(out, "- 关联证据：{}", manual.evidence_refs.join(" / "))?;
        if manual.state == ConfirmationState::Pending {
            writeln!(out, "- 状态：待人工确认\n")?;
        } else {
            writeln!(
                out,
                "- 审阅人：{}",
                manual.reviewer.as_deref().unwrap_or("未署名")
            )?;
            writeln!(
                out,
                "- 说明：{}",
                manual.note.as_deref().unwrap_or("未填写")
            )?;
            writeln!(
                out,
                "- 时间：{}\n",
                manual.decided_at.as_deref().unwrap_or("未记录")
            )?;
        }
    }

    if let Some(replay) = &report.replay {
        writeln!(out, "## 恢复后的重放\n")?;
        writeln!(out, "- 对象：`{}`", replay.source_id)?;
        writeln!(out, "- 触发：{}", replay.trigger)?;
        writeln!(out, "- 检查点：`{}`", replay.checkpoint_id)?;
        writeln!(out, "- 原始输入：`{}`", replay.original_input)?;
        writeln!(out, "- 原始 SHA-256：`{}`", replay.original_sha256)?;
        writeln!(out, "- 重放 SHA-256：`{}`", replay.replay_sha256)?;
        writeln!(out, "- 比较字段：{}", replay.compared_fields.join(" / "))?;
        writeln!(
            out,
            "- 是否一致：{}\n",
            if replay.identical { "是" } else { "否" }
        )?;
        for step in &replay.steps {
            writeln!(
                out,
                "  - S{:02} {}：{} → {} [{}]",
                step.seq,
                step.action,
                step.input,
                step.output,
                step_state_label(step.state)
            )?;
        }
        out.push('\n');
    }

    writeln!(out, "## 失败处置\n")?;
    if report.failures.is_empty() {
        writeln!(out, "无。\n")?;
    }
    for failure in &report.failures {
        writeln!(out, "### {} · {}\n", failure.code, failure.source_id)?;
        writeln!(out, "{}\n", failure.summary)?;
        for (index, action) in failure.next_actions.iter().enumerate() {
            writeln!(
                out,
                "{}. **{}** — {}",
                index + 1,
                action.label,
                action.detail
            )?;
            if let Some(command) = &action.command {
                writeln!(out, "   ```sh\n   {command}\n   ```")?;
            }
        }
        out.push('\n');
    }

    writeln!(out, "## 证据索引\n")?;
    for source in &report.sources {
        writeln!(out, "### {} · {}\n", source.spec.id, source.spec.label)?;
        writeln!(out, "- 原始输入：`{}`", source.spec.path)?;
        writeln!(out, "- 来源：{}", source.spec.origin)?;
        writeln!(out, "- 状态：{}", source.state.label())?;
        if let Some(probe) = &source.probe {
            writeln!(out, "- SHA-256：`{}`", probe.sha256)?;
            writeln!(
                out,
                "- 字节窗口：head {} B / tail {} B",
                probe.head_hex.split_whitespace().count(),
                probe.tail_hex.split_whitespace().count()
            )?;
        }
        writeln!(out, "- 处理步骤：")?;
        for step in &source.steps {
            writeln!(
                out,
                "  - S{:02} {}：{} → {} [{}]",
                step.seq,
                step.action,
                step.input,
                step.output,
                step_state_label(step.state)
            )?;
        }
        writeln!(out, "- 证据文件：")?;
        for evidence in &source.evidence {
            writeln!(
                out,
                "  - `{}` — {}（{}）",
                evidence.path, evidence.label, evidence.note
            )?;
        }
        out.push('\n');
    }

    writeln!(out, "## 审计边界\n")?;
    writeln!(
        out,
        "- 未知扩展不使用同类 MIME 或外观猜测；人工确认不能替代原始字节。"
    )?;
    writeln!(
        out,
        "- 截断残片只确认签名字节，不声明完整媒体或可交付边界。"
    )?;
    writeln!(
        out,
        "- 恢复重放比较同一路径、同一检查点快照；字段不一致时不得自动通过。"
    )?;
    writeln!(
        out,
        "- 源文件缺失时保留缺失记录，不生成占位哈希，也不以同名文件替代。"
    )?;

    let path = root.join("artifacts/report.md");
    atomic_write(&path, out.as_bytes())?;
    Ok(path.display().to_string())
}

fn finding_kind_label(kind: FindingKind) -> &'static str {
    match kind {
        FindingKind::Pass => "PASS",
        FindingKind::Unresolved => "UNRESOLVED",
        FindingKind::Ambiguous => "AMBIGUOUS",
        FindingKind::Failure => "FAILURE",
        FindingKind::ReplayConsistent => "REPLAY",
    }
}

fn manual_state_label(state: ConfirmationState) -> &'static str {
    match state {
        ConfirmationState::Pending => "待人工",
        ConfirmationState::Confirmed => "已确认",
        ConfirmationState::Rejected => "已排除",
    }
}

fn step_state_label(state: StepState) -> &'static str {
    match state {
        StepState::Ok => "OK",
        StepState::Warning => "CHECK",
        StepState::Failed => "STOP",
        StepState::Skipped => "SKIP",
    }
}

#[allow(dead_code)]
fn source_state_label(state: SourceState) -> &'static str {
    state.label()
}
