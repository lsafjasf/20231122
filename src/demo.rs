//! Deterministic first-run demo fixture.

use crate::model::{
    APP_VERSION, Device, DeviceId, DuplicateObject, EvidenceKind, SCHEMA_VERSION, Segment,
    SegmentStatus, Session, field,
};
use crate::util::{now_rfc3339, sha256_file};
use anyhow::{Context, Result};
use std::path::Path;

pub fn seed_session(base_dir: &Path) -> Session {
    let media_dir = base_dir.join("demo");
    let source_a = media_dir
        .join("device-a.webm")
        .to_string_lossy()
        .into_owned();
    let source_b = media_dir
        .join("device-b.webm")
        .to_string_lossy()
        .into_owned();
    let now = now_rfc3339();
    let mut session = Session {
        schema_version: SCHEMA_VERSION,
        session_id: "case-duplicate-0039".to_owned(),
        app_version: APP_VERSION.to_owned(),
        created_at: now.clone(),
        updated_at: now,
        duplicate_object: DuplicateObject {
            object_id: "OBJ-2026-0039".to_owned(),
            label: "重复对象 0039 · 双机位对照".to_owned(),
            note: "两台设备分别记录同一对象；时钟存在不同步，仅对受影响片段做局部优先级修正。"
                .to_owned(),
        },
        devices: [
            Device {
                id: DeviceId::A,
                name: "A 机 · 主视角".to_owned(),
                clock_offset_ms: 1_500,
                clock_note: "A 机时钟比共享基准快 1.5 秒".to_owned(),
            },
            Device {
                id: DeviceId::B,
                name: "B 机 · 副视角".to_owned(),
                clock_offset_ms: -800,
                clock_note: "B 机时钟比共享基准慢 0.8 秒".to_owned(),
            },
        ],
        segments: Vec::new(),
        runs: Vec::new(),
        evidence: Vec::new(),
        next_evidence_seq: 1,
        next_run_seq: 1,
    };
    session.segments = vec![
        segment(
            1,
            "seg-01",
            "A 机 / 入场",
            DeviceId::A,
            &source_a,
            2_000,
            0,
            2_400,
            40,
        ),
        segment(
            2,
            "seg-02",
            "B 机 / 入场",
            DeviceId::B,
            &source_b,
            900,
            0,
            2_400,
            40,
        ),
        segment(
            3,
            "seg-03",
            "A 机 / 交接",
            DeviceId::A,
            &source_a,
            7_800,
            3_000,
            5_800,
            30,
        ),
        segment(
            4,
            "seg-04",
            "B 机 / 交接",
            DeviceId::B,
            &source_b,
            7_100,
            3_000,
            5_800,
            25,
        ),
        segment(
            5,
            "seg-05",
            "A 机 / 离场",
            DeviceId::A,
            &source_a,
            13_600,
            6_500,
            9_000,
            20,
        ),
        segment(
            6,
            "seg-06",
            "B 机 / 离场",
            DeviceId::B,
            &source_b,
            12_900,
            6_500,
            9_000,
            20,
        ),
    ];
    session.append_evidence(
        EvidenceKind::SessionCreated,
        None,
        None,
        "初始化重复对象双设备会话",
        vec![
            field("object_id", &session.duplicate_object.object_id),
            field("device_a_offset_ms", session.devices[0].clock_offset_ms),
            field("device_b_offset_ms", session.devices[1].clock_offset_ms),
            field("segment_count", session.segments.len()),
            field(
                "order_policy",
                "segment.ordinal ascending; queue priority descending",
            ),
        ],
    );
    session
}

pub fn ensure_demo_media(session: &mut Session) -> Result<()> {
    let mut changed = false;
    for segment in &mut session.segments {
        let path = Path::new(&segment.source_path);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("创建示例目录失败：{}", parent.display()))?;
            }
            crate::transcode::generate_demo_clip(path, segment.device)
                .with_context(|| format!("生成示例片段失败：{}", path.display()))?;
        }
        let digest = sha256_file(path).ok();
        if digest != segment.source_sha256 {
            segment.source_sha256 = digest;
            changed = true;
        }
    }
    if changed {
        session.touch();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn segment(
    ordinal: u32,
    id: &str,
    label: &str,
    device: DeviceId,
    source_path: &str,
    capture_start_ms: i64,
    in_ms: i64,
    out_ms: i64,
    base_priority: i32,
) -> Segment {
    Segment {
        ordinal,
        id: id.to_owned(),
        label: label.to_owned(),
        device,
        source_path: source_path.to_owned(),
        source_sha256: None,
        capture_start_ms,
        in_ms,
        out_ms,
        base_priority,
        priority_override: None,
        status: SegmentStatus::Pending,
        latest_run_id: None,
        correction_note: String::new(),
    }
}
