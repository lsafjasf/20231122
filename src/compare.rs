use anyhow::{Context, Result, bail};
use chrono::{SecondsFormat, Utc};
use uuid::Uuid;

use crate::model::{AttemptRecord, AttemptStatus, ComparisonRecord, Segment};

pub fn compare_attempts(left: &AttemptRecord, right: &AttemptRecord) -> ComparisonRecord {
    let size_delta_bytes = match (left.output_size_bytes, right.output_size_bytes) {
        (Some(left), Some(right)) => Some(left as i64 - right as i64),
        _ => None,
    };
    let checksum_equal = matches!(
        (&left.output_sha256, &right.output_sha256),
        (Some(left_hash), Some(right_hash)) if left_hash == right_hash
    );
    let same_source = left.device_id == right.device_id && left.source_path == right.source_path;
    let verdict = verdict_for(left, right, checksum_equal, same_source, size_delta_bytes);
    ComparisonRecord {
        id: format!("comparison-{}", Uuid::new_v4().simple()),
        created_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        left_attempt_id: left.id.clone(),
        right_attempt_id: right.id.clone(),
        left_status: left.status,
        right_status: right.status,
        left_size_bytes: left.output_size_bytes,
        right_size_bytes: right.output_size_bytes,
        size_delta_bytes,
        left_duration_ms: left.duration_ms,
        right_duration_ms: right.duration_ms,
        checksum_equal,
        same_source,
        verdict,
    }
}

fn verdict_for(
    left: &AttemptRecord,
    right: &AttemptRecord,
    checksum_equal: bool,
    same_source: bool,
    size_delta_bytes: Option<i64>,
) -> String {
    if left.status != AttemptStatus::Succeeded || right.status != AttemptStatus::Succeeded {
        return format!(
            "至少一侧未成功完成（{} / {}），不能据此判定画面修复结果；请先查看失败证据。",
            left.status.label(),
            right.status.label()
        );
    }
    if checksum_equal {
        return "两侧输出逐字节一致；本次重跑可复现，校验和相同。".to_string();
    }
    let size_note = size_delta_bytes
        .map(|delta| format!("大小差 {delta:+} 字节"))
        .unwrap_or_else(|| "大小不可用".to_string());
    if same_source {
        format!(
            "同一源设备重跑：{size_note}，但校验和不同。容器时间戳或编码元数据可能变化，需播放抽帧确认内容。"
        )
    } else {
        format!(
            "跨设备对齐重跑：{size_note}，校验和不同。两路源本身不同，必须结合时间码与抽帧人工复核。"
        )
    }
}

pub fn average_wall_time_ms(attempts: &[AttemptRecord]) -> Option<u64> {
    let successful: Vec<_> = attempts
        .iter()
        .filter(|attempt| attempt.status == AttemptStatus::Succeeded)
        .collect();
    if successful.is_empty() {
        return None;
    }
    let total: u64 = successful.iter().map(|attempt| attempt.wall_time_ms).sum();
    Some(total / successful.len() as u64)
}

pub fn store_comparison(segment: &mut Segment, comparison: ComparisonRecord) -> Result<()> {
    if !segment
        .attempts
        .iter()
        .any(|attempt| attempt.id == comparison.left_attempt_id)
    {
        bail!("比较记录引用了不存在的左侧尝试");
    }
    if !segment
        .attempts
        .iter()
        .any(|attempt| attempt.id == comparison.right_attempt_id)
    {
        bail!("比较记录引用了不存在的右侧尝试");
    }
    segment.comparisons.push(comparison);
    Ok(())
}

pub fn find_attempt<'a>(segment: &'a Segment, attempt_id: &str) -> Result<&'a AttemptRecord> {
    segment
        .attempts
        .iter()
        .find(|attempt| attempt.id == attempt_id)
        .with_context(|| format!("尝试不存在: {attempt_id}"))
}
