//! 端到端：生成媒体 → 分析 → 截断 → 恢复 → 重放，
//! 验证第一个错误状态可被精确定位（v1/v2 两种格式）。
use safe_frame_audit::detect;
use safe_frame_audit::genmedia;
use safe_frame_audit::params::SafeFrameParams;
use safe_frame_audit::record;
use safe_frame_audit::replay;
use std::path::PathBuf;
const FRAMES: u64 = 80;
fn workdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("sfar-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
/// 用生成器同源逻辑离线计算期望的首错帧号。
fn expected_first_error(params: &SafeFrameParams) -> u64 {
    (0..FRAMES)
        .find(|&f| {
            let rgb = genmedia::synthetic_frame(f);
            detect::metric_for_frame(&rgb, genmedia::GEN_W, genmedia::GEN_H, params)
                > params.max_ratio
        })
        .expect("测试媒体必须产生违规帧")
}
#[test]
fn full_reviewable_flow_v2_and_v1() {
    let dir = workdir();
    let media = dir.join("sample.mkv");
    let params = SafeFrameParams::default();
    genmedia::generate(&media, FRAMES).unwrap();
    let expect_fe = expected_first_error(&params);
    // ---- v2 新格式：分析 → 截断 → 恢复 → 重放 ----
    let rec_v2 = dir.join("sample.sfar2.jsonl");
    let metrics = {
        let mut writer =
            record::WriterV2::create(&rec_v2, &media.to_string_lossy(), &params).unwrap();
        let ms = detect::analyze(&media, &params, |fm, _rgb, _w, _h| {
            writer
                .push(fm.pts_ms, fm.frame, "video0", fm.metric, fm.violation)
                .unwrap();
        })
        .unwrap();
        writer.finish().unwrap();
        ms
    };
    assert_eq!(metrics.len() as u64, FRAMES);
    let rec = record::load(&rec_v2).unwrap();
    assert_eq!(rec.wave_crc_ok, Some(true));
    let fe = rec.first_error().expect("必须有首错帧");
    assert_eq!(fe.frame, expect_fe, "首错帧必须与离线期望一致");
    assert_eq!(fe.pts_ms, (expect_fe * 100) as i64, "时间点必须对应帧号");
    assert_eq!(fe.track, "video0");
    // 模拟崩溃截断（切进 eof 行中间）
    let raw = std::fs::read(&rec_v2).unwrap();
    let cut_len = raw.len() - 19;
    std::fs::write(&rec_v2, &raw[..cut_len]).unwrap();
    let rec = record::load(&rec_v2).unwrap();
    assert!(rec.tail_garbage_bytes > 0, "截断后必须能检出垃圾尾");
    let mark = record::recover(&rec_v2).unwrap();
    assert!(mark.dropped_bytes > 0);
    // 关键证据：截断原件被保留
    let bak = dir.join("sample.sfar2.jsonl.truncated.bak");
    assert!(bak.exists());
    assert_eq!(std::fs::read(&bak).unwrap(), raw[..cut_len].to_vec());
    // 恢复后的重放
    let rep = replay::replay(&rec_v2, None).unwrap();
    assert_eq!(rep.verdict, "OK", "恢复后重放必须一致: {rep:?}");
    assert!(rep.was_recovered);
    assert_eq!(rep.first_error.unwrap().frame, expect_fe);
    assert!(rep.first_mismatch.is_none());
    // ---- v1 旧格式：兼容读取 + 截断恢复 + 重放定位缺失点 ----
    let rec_v1 = dir.join("legacy.sfar1.jsonl");
    let entries: Vec<record::Entry> = metrics
        .iter()
        .map(|m| record::Entry {
            seq: m.frame,
            pts_ms: m.pts_ms,
            frame: m.frame,
            track: "video0".into(),
            metric: m.metric,
            violation: m.violation,
            params_hash: String::new(),
        })
        .collect();
    record::write_v1(&rec_v1, &entries).unwrap();
    let raw = std::fs::read(&rec_v1).unwrap();
    std::fs::write(&rec_v1, &raw[..raw.len() - 13]).unwrap();
    record::recover(&rec_v1).unwrap();
    let rec = record::load(&rec_v1).unwrap();
    assert_eq!(rec.format, record::Format::V1);
    assert!(rec.recovered.is_some());
    assert_eq!(rec.first_error().unwrap().frame, expect_fe, "v1 首错帧一致");
    // v1 记录丢了最后一帧 → 重放必须指出第一个缺失点而不是只给总分
    let rep = replay::replay(&rec_v1, Some(&media)).unwrap();
    assert_eq!(rep.verdict, "MISMATCH");
    let mm = rep.first_mismatch.expect("必须报告第一个不一致");
    assert_eq!(mm.kind, "missing_in_record");
    assert_eq!(mm.frame, FRAMES - 1);
}
