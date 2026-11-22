use std::fmt::Write as _;
use std::path::Path;

use crate::model::{Processor, RepairReport, VersionResult};
use crate::util::csv_field;

pub const CSV_HEADER: &str = "version_index,version_id,version_label,interval_order,interval_id,source_device,requested_start_ms,requested_end_ms,canonical_start_ms,canonical_end_ms,applied_start_frame,applied_end_frame,track_id,source_start_frame_delta,source_end_frame_delta,processor_kind,processor_parameters,applied,disposition,output_path,output_sha256,waveform_path,waveform_sha256";

pub fn markdown(report: &RepairReport) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "# 音频修复版本复核记录\n");
    let _ = writeln!(output, "- 运行 ID：`{}`", report.run_id);
    let _ = writeln!(output, "- 创建时间：`{}`", report.created_at);
    let _ = writeln!(output, "- 状态：`{:?}`", report.status);
    let _ = writeln!(
        output,
        "- 工具：`{} {}`，Rust `{}`，GStreamer `{}`，GTK4 `{}`\n",
        report.application.name,
        report.application.version,
        report.application.rust,
        report.application.gstreamer,
        report.application.gtk4
    );

    let _ = writeln!(output, "## 独立复核步骤\n");
    let artifact_dir = Path::new(&report.integrity.report_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let _ = writeln!(output, "```sh");
    let _ = writeln!(
        output,
        "cd {}",
        shell_quote(&artifact_dir.display().to_string())
    );
    let _ = writeln!(output, "sha256sum -c SHA256SUMS");
    let _ = writeln!(
        output,
        "audio-repair-cli verify {}",
        shell_quote(&artifact_dir.display().to_string())
    );
    let _ = writeln!(output, "```\n");
    let _ = writeln!(
        output,
        "校验器会重新计算所有产物的 SHA-256，并核对版本顺序、区间顺序、输出与波形关联。原始输入不在产物目录时，会报告缺失但不会伪造通过。\n"
    );

    let _ = writeln!(output, "## 输入证据\n");
    let _ = writeln!(output, "| 输入 | 路径 | 字节数 | SHA-256 |");
    let _ = writeln!(output, "|---|---|---:|---|");
    for (label, file) in [
        ("原始录音", &report.inputs.recording),
        ("修复区间", &report.inputs.intervals),
        ("版本清单", &report.inputs.versions),
    ] {
        let _ = writeln!(
            output,
            "| {} | `{}` | {} | `{}` |",
            label,
            md_escape(&file.path),
            file.bytes,
            file.sha256
        );
    }

    let _ = writeln!(output, "\n## 时钟映射\n");
    let _ = writeln!(
        output,
        "统一时间轴公式：`canonical_ms=(device_ms-offset_ms)/(1+drift_ppm/1000000)`。输出帧率为 {} Hz。\n",
        report.media.decoded_sample_rate
    );
    let _ = writeln!(
        output,
        "| 设备 | 角色 | offset_ms | drift_ppm | 最大观测偏差 ms | 证据区间 | 警告 |"
    );
    let _ = writeln!(output, "|---|---|---:|---:|---:|---|---|");
    for clock in &report.clocks {
        let _ = writeln!(
            output,
            "| `{}` {} | {} | {:.6} | {:.6} | {:.6} | {} | {} |",
            clock.device_id,
            md_escape(&clock.label),
            clock.role,
            clock.offset_ms,
            clock.drift_ppm,
            clock.max_observed_skew_ms,
            if clock.max_observed_skew_interval.is_empty() {
                "-".to_string()
            } else {
                format!("`{}`", clock.max_observed_skew_interval)
            },
            clock.warning.as_deref().unwrap_or("-")
        );
    }

    let _ = writeln!(output, "\n## 区间与帧对应\n");
    let _ = writeln!(
        output,
        "| 区间 | 设备/轨道 | 请求起止 ms | 统一时间起止 ms | 应用帧 | 时钟偏差 ms | 状态 | 警告 |"
    );
    let _ = writeln!(output, "|---|---|---|---|---|---:|---|---|");
    for interval in &report.intervals {
        let _ = writeln!(
            output,
            "| `{}` | `{}` / {} {} | {:.6} .. {:.6} | {:.6} .. {:.6} | {} .. {} | {:.6} | {} | {} |",
            interval.interval_id,
            interval.device_id,
            interval.track_id,
            md_escape(&interval.track_label),
            interval.requested_start_ms,
            interval.requested_end_ms,
            interval.canonical_start_ms,
            interval.canonical_end_ms,
            interval.applied_start_frame,
            interval.applied_end_frame,
            interval.clock_skew_ms,
            interval.disposition,
            if interval.warnings.is_empty() {
                "-".to_string()
            } else {
                md_escape(&interval.warnings.join("; "))
            }
        );
    }

    let _ = writeln!(output, "\n## 版本与输出\n");
    for version in &report.versions {
        render_version(&mut output, version, report.media.decoded_sample_rate);
    }

    let _ = writeln!(output, "## 证据流水\n");
    let _ = writeln!(output, "| # | 类型 | 级别 | 对象 | 说明 |");
    let _ = writeln!(output, "|---:|---|---|---|---|");
    for entry in &report.evidence {
        let _ = writeln!(
            output,
            "| {} | {} | {} | {} | {} |",
            entry.sequence,
            entry.kind,
            entry.severity,
            md_escape(&entry.subject),
            md_escape(&entry.message)
        );
    }
    output
}

fn render_version(output: &mut String, version: &VersionResult, sample_rate: u32) {
    let _ = writeln!(output, "### {}: {}\n", version.version_id, version.label);
    let _ = writeln!(output, "- 清单序号：{}", version.manifest_index);
    let _ = writeln!(output, "- 备注：{}", md_escape(&version.note));
    let _ = writeln!(output, "- 输出：`{}`", version.output_path);
    let _ = writeln!(
        output,
        "- 输出 SHA-256：`{}`（{} 字节）",
        version.output_sha256, version.output_bytes
    );
    let _ = writeln!(output, "- 波形：`{}`", version.waveform_path);
    let _ = writeln!(output, "- 波形 SHA-256：`{}`", version.waveform_sha256);
    let _ = writeln!(
        output,
        "- 波形摘要：{} 桶，桶宽 {:.3} ms，peak={:.6}，rms={:.6}，min={:.6}，max={:.6}",
        version.waveform.bucket_count,
        version.waveform.bucket_ms,
        version.waveform.peak,
        version.waveform.rms,
        version.waveform.min,
        version.waveform.max
    );
    let _ = writeln!(
        output,
        "- 版本引用顺序：{}",
        version
            .interval_ids
            .iter()
            .map(|id| format!("`{id}`"))
            .collect::<Vec<_>>()
            .join(" -> ")
    );
    let _ = writeln!(output, "- 实际应用顺序与帧：");
    for applied in &version.applied_intervals {
        let _ = writeln!(
            output,
            "  {}. `{}` @ {}..{}，{}",
            applied.manifest_order + 1,
            applied.interval_id,
            applied.start_frame,
            applied.end_frame,
            processor_label(&applied.processor)
        );
    }
    if version.interval_ids.is_empty() {
        let _ = writeln!(output, "  - 无修复区间，输出为规范化后的原始音频。");
    }
    let duration_seconds = version.waveform.source_frame_count as f64 / sample_rate as f64;
    let _ = writeln!(
        output,
        "- 输出样本数：{}（{:.6} 秒）",
        version.waveform.source_frame_count, duration_seconds
    );
    if !version.warnings.is_empty() {
        let _ = writeln!(
            output,
            "- 警告：{}",
            md_escape(&version.warnings.join("; "))
        );
    }
    let _ = writeln!(output);
}

pub fn csv(report: &RepairReport) -> String {
    let mut output = String::from(CSV_HEADER);
    output.push('\n');
    for (version_index, version) in report.versions.iter().enumerate() {
        if version.interval_ids.is_empty() {
            write_csv_row(&mut output, version_index, version, None, report);
        } else {
            for (order, interval_id) in version.interval_ids.iter().enumerate() {
                write_csv_row(
                    &mut output,
                    version_index,
                    version,
                    Some((order, interval_id)),
                    report,
                );
            }
        }
    }
    output
}

fn write_csv_row(
    output: &mut String,
    version_index: usize,
    version: &VersionResult,
    reference: Option<(usize, &String)>,
    report: &RepairReport,
) {
    let applied = reference.and_then(|(_, interval_id)| {
        version
            .applied_intervals
            .iter()
            .find(|item| &item.interval_id == interval_id)
    });
    let interval_id = reference.map(|(_, interval_id)| interval_id.as_str());
    let interval = interval_id.and_then(|interval_id| {
        report
            .intervals
            .iter()
            .find(|candidate| candidate.interval_id == interval_id)
    });
    let order = reference.map(|(order, _)| order);
    let applied_start = applied
        .map(|item| item.start_frame)
        .or_else(|| interval.map(|item| item.applied_start_frame));
    let applied_end = applied
        .map(|item| item.end_frame)
        .or_else(|| interval.map(|item| item.applied_end_frame));
    let processor = applied
        .map(|item| &item.processor)
        .or_else(|| interval.map(|item| &item.processor));
    let fields = vec![
        version_index.to_string(),
        version.version_id.clone(),
        version.label.clone(),
        order.map(|value| value.to_string()).unwrap_or_default(),
        interval_id.unwrap_or_default().to_string(),
        interval
            .map(|item| item.device_id.clone())
            .unwrap_or_default(),
        interval
            .map(|item| format!("{:.6}", item.requested_start_ms))
            .unwrap_or_default(),
        interval
            .map(|item| format!("{:.6}", item.requested_end_ms))
            .unwrap_or_default(),
        interval
            .map(|item| format!("{:.6}", item.canonical_start_ms))
            .unwrap_or_default(),
        interval
            .map(|item| format!("{:.6}", item.canonical_end_ms))
            .unwrap_or_default(),
        applied_start
            .map(|value| value.to_string())
            .unwrap_or_default(),
        applied_end
            .map(|value| value.to_string())
            .unwrap_or_default(),
        interval
            .map(|item| item.track_id.to_string())
            .unwrap_or_default(),
        interval
            .map(|item| item.source_frame_delta.to_string())
            .unwrap_or_default(),
        interval
            .map(|item| item.end_source_frame_delta.to_string())
            .unwrap_or_default(),
        processor
            .map(processor_kind)
            .map(str::to_string)
            .unwrap_or_default(),
        processor.map(processor_parameters).unwrap_or_default(),
        applied
            .filter(|_| interval.is_some_and(|item| item.applied))
            .map(|_| "true".to_string())
            .unwrap_or_else(|| "false".to_string()),
        interval
            .map(|item| item.disposition.clone())
            .unwrap_or_default(),
        version.output_path.clone(),
        version.output_sha256.clone(),
        version.waveform_path.clone(),
        version.waveform_sha256.clone(),
    ];
    output.push_str(
        &fields
            .iter()
            .map(|field| csv_field(field))
            .collect::<Vec<_>>()
            .join(","),
    );
    output.push('\n');
}

pub fn processor_label(processor: &Processor) -> String {
    format!(
        "{}({})",
        processor_kind(processor),
        processor_parameters(processor)
    )
}

pub fn processor_kind(processor: &Processor) -> &'static str {
    match processor {
        Processor::Gain { .. } => "gain",
        Processor::Mute { .. } => "mute",
        Processor::Declick => "declick",
        Processor::Normalize { .. } => "normalize",
        Processor::ReplaceSilence { .. } => "replace_silence",
    }
}

pub fn processor_parameters(processor: &Processor) -> String {
    match processor {
        Processor::Gain { gain_db, fade_ms } => {
            format!("gain_db={gain_db:.6};fade_ms={fade_ms:.6}")
        }
        Processor::Mute { fade_ms } | Processor::ReplaceSilence { fade_ms } => {
            format!("fade_ms={fade_ms:.6}")
        }
        Processor::Declick => "method=linear_bridge".to_string(),
        Processor::Normalize {
            target_peak_db,
            fade_ms,
        } => format!("target_peak_db={target_peak_db:.6};fade_ms={fade_ms:.6}"),
    }
}

fn md_escape(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_parameters_are_stable_and_ordered() {
        let gain = Processor::Gain {
            gain_db: -3.0,
            fade_ms: 12.0,
        };
        assert_eq!(
            processor_parameters(&gain),
            "gain_db=-3.000000;fade_ms=12.000000"
        );
    }
}
