use syncwall::analyze::analyze;
use syncwall::journal::JournalWriter;
use syncwall::model::{RawEvent, Verdict};

fn write_session(w: &mut JournalWriter, chans: u32, seqs: std::ops::Range<u64>, base: u64) {
    w.write_event(&RawEvent::SessionStart {
        session_id: "t".into(),
        channels: chans,
        tick_ms: 33,
        started_ms: 0,
    })
    .unwrap();
    for seq in seqs {
        for channel in 0..chans {
            w.write_event(&RawEvent::Frame {
                channel,
                seq,
                pts_ms: base + seq * 33,
                wall_ms: base + seq * 33,
            })
            .unwrap();
        }
    }
}

#[test]
fn truncated_then_replay_has_boundary_and_first_error() {
    let dir = std::env::temp_dir().join(format!("sw-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let orig = dir.join("orig.jsonl");
    let replay = dir.join("replay.jsonl");

    // 原始记录：seq 0..20，通道 1 丢 seq 7，seq 20 处截断
    let mut w = JournalWriter::create(&orig).unwrap();
    write_session(&mut w, 2, 0..7, 1000);
    for seq in 7..20 {
        for channel in 0..2 {
            if channel == 1 && seq == 7 {
                continue; // 丢帧
            }
            w.write_event(&RawEvent::Frame {
                channel,
                seq,
                pts_ms: 1000 + seq * 33,
                wall_ms: 1000 + seq * 33,
            })
            .unwrap();
        }
    }
    w.write_partial(&RawEvent::Frame {
        channel: 0,
        seq: 20,
        pts_ms: 1660,
        wall_ms: 1660,
    })
    .unwrap();
    drop(w);

    // 恢复重放：从 seq 20 续采
    let mut w2 = JournalWriter::create(&replay).unwrap();
    w2.write_event(&RawEvent::Recovered {
        session_id: "replay".into(),
        resumed_from: "orig.jsonl".into(),
    })
    .unwrap();
    for seq in 20..25 {
        for channel in 0..2 {
            w2.write_event(&RawEvent::Frame {
                channel,
                seq,
                pts_ms: 2000 + seq * 33,
                wall_ms: 2000 + seq * 33,
            })
            .unwrap();
        }
    }
    drop(w2);

    let report = analyze(&[&orig, &replay]).unwrap();

    // 截断被识别
    assert!(report.findings.iter().any(|f| f.kind == "record-truncated"));
    // 无法判断的边界被保留
    let boundary: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.verdict == Verdict::Indeterminate)
        .collect();
    assert_eq!(boundary.len(), 2, "每通道一个边界结论");
    assert!(boundary[0].evidence.len() >= 3, "边界证据链完整");
    // 第一个错误状态是截断还是丢帧取决于时间；丢帧应有 at_ms
    let fe = report.first_error.expect("应有首个错误");
    let fe_finding = &report.findings[fe];
    assert_eq!(fe_finding.verdict, Verdict::Error);
    // 丢帧结论存在且证据指向原始行
    let lost = report
        .findings
        .iter()
        .find(|f| f.kind == "frame-lost")
        .expect("应检出通道 1 丢帧");
    assert_eq!(lost.channel, Some(1));
    assert!(lost.evidence.iter().all(|e| e.line_no > 0));
    // 处理步骤完整
    assert!(report.steps.len() >= 5);
}

#[test]
fn clean_session_has_no_error() {
    let dir = std::env::temp_dir().join(format!("swc-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("clean.jsonl");
    let mut w = JournalWriter::create(&p).unwrap();
    write_session(&mut w, 3, 0..30, 5000);
    drop(w);
    let report = analyze(&[&p]).unwrap();
    assert_eq!(report.error_count, 0);
    assert!(report.first_error.is_none());
    assert_eq!(report.channels.len(), 3);
}
