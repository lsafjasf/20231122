//! 核心规则/修复/报告测试：正常帧、缺字段帧、互相冲突的帧。

use hdr_meta_fix::fix::RepairSession;
use hdr_meta_fix::meta::*;
use hdr_meta_fix::rules::{self, IssueKind};
use hdr_meta_fix::{report, synth};

#[test]
fn normal_frames_have_no_issues() {
    for f in synth::normal_frames() {
        let issues = rules::analyze(&f);
        assert!(issues.is_empty(), "帧 {} 不应有问题：{issues:?}", f.frame_no);
    }
}

#[test]
fn missing_frames_report_missing_fields() {
    let frames = synth::missing_field_frames();
    // 帧 10：声明 HDR10 但缺 primaries/matrix/mastering/cll/fall。
    let issues = rules::analyze(&frames[0]);
    assert!(issues.iter().all(|i| i.kind == IssueKind::Missing));
    let missing_fields: Vec<FieldKey> =
        issues.iter().flat_map(|i| i.fields.clone()).collect();
    for expected in [
        FieldKey::Primaries,
        FieldKey::Matrix,
        FieldKey::MasteringMaxLum,
        FieldKey::MaxCll,
        FieldKey::MaxFall,
    ] {
        assert!(missing_fields.contains(&expected), "帧 10 应报缺 {expected:?}");
    }
    // 帧 11：PQ 传递函数但缺 HDR 格式标签。
    let issues = rules::analyze(&frames[1]);
    assert!(issues
        .iter()
        .any(|i| i.kind == IssueKind::Missing && i.fields == [FieldKey::HdrFormat]));
}

#[test]
fn conflicting_frames_report_conflicts() {
    let frames = synth::conflicting_frames();
    // 帧 20：HDR10 标签 vs HLG 传递函数；HDR 传递函数 vs bt709 矩阵。
    let issues = rules::analyze(&frames[0]);
    assert!(issues.iter().any(|i| i.kind == IssueKind::Conflict
        && i.fields.contains(&FieldKey::HdrFormat)
        && i.fields.contains(&FieldKey::Transfer)));
    assert!(issues.iter().any(|i| i.fields.contains(&FieldKey::Matrix)));
    // 帧 21：MaxFALL > MaxCLL，MaxCLL > 母版峰值。
    let issues = rules::analyze(&frames[1]);
    assert!(issues.iter().any(|i| i.fields.contains(&FieldKey::MaxFall)
        && i.fields.contains(&FieldKey::MaxCll)));
    assert!(issues.iter().any(|i| i.fields.contains(&FieldKey::MaxCll)
        && i.fields.contains(&FieldKey::MasteringMaxLum)));
    // 帧 22：EV 隐含亮度 vs 母版峰值；BT.2020 CL vs full range。
    let issues = rules::analyze(&frames[2]);
    assert!(issues.iter().any(|i| i.fields.contains(&FieldKey::ExposureValue)));
    assert!(issues.iter().any(|i| i.fields.contains(&FieldKey::Range)));
    // 帧 23：SDR 标签 vs PQ 传递函数。
    let issues = rules::analyze(&frames[3]);
    assert!(issues.iter().any(|i| i.kind == IssueKind::Conflict
        && i.fields.contains(&FieldKey::HdrFormat)));
}

#[test]
fn apply_and_undo_field() {
    let mut session = RepairSession::new(synth::conflicting_frames());
    let issues = session.issues(0);
    let sug = issues
        .iter()
        .flat_map(|i| i.suggestions.iter())
        .find(|s| s.field == FieldKey::Transfer)
        .cloned()
        .expect("帧 20 应有传递函数建议");
    assert!(session.apply(0, &sug));
    assert_eq!(
        session.frames[0].get(FieldKey::Transfer),
        Some(FieldValue::Transfer(Transfer::Pq))
    );
    assert!(session.frames[0].is_modified(FieldKey::Transfer));
    // 撤销该字段：恢复原始值 HLG。
    assert!(session.undo_field(0, FieldKey::Transfer));
    assert_eq!(
        session.frames[0].get(FieldKey::Transfer),
        Some(FieldValue::Transfer(Transfer::Hlg))
    );
    // 再应用后用全局撤销。
    assert!(session.apply(0, &sug));
    assert!(session.undo_last());
    assert_eq!(
        session.frames[0].get(FieldKey::Transfer),
        Some(FieldValue::Transfer(Transfer::Hlg))
    );
}

#[test]
fn applying_all_suggestions_converges() {
    let mut session = RepairSession::new(synth::sample_frames());
    for idx in 0..session.frames.len() {
        for _ in 0..20 {
            let issues = session.issues(idx);
            let Some(sug) = issues.iter().flat_map(|i| i.suggestions.iter()).next().cloned()
            else {
                break;
            };
            if !session.apply(idx, &sug) {
                break;
            }
        }
    }
    assert_eq!(session.remaining_issues(), 0, "全部建议应用后应无剩余问题");
    assert!(!session.edits().is_empty());
}

#[test]
fn report_contains_frame_field_and_rationale() {
    let mut session = RepairSession::new(synth::conflicting_frames());
    let issues = session.issues(1); // 帧 21
    let sug = issues
        .iter()
        .flat_map(|i| i.suggestions.iter())
        .find(|s| s.field == FieldKey::MaxFall)
        .cloned()
        .unwrap();
    session.apply(1, &sug);
    let md = report::to_markdown(&session);
    assert!(md.contains("| 21 |"), "报告应含帧号：\n{md}");
    assert!(md.contains(FieldKey::MaxFall.label()), "报告应含字段名：\n{md}");
    assert!(md.contains("MaxFALL 按定义不超过 MaxCLL"), "报告应含修复依据：\n{md}");
    let csv = report::to_csv(&session);
    assert!(csv.contains("21,"), "CSV 应含帧号：\n{csv}");
}
