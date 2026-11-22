//! 时间轴合并判定：逐条给出结论，并让每个结论都能回到原始输入。

use crate::model::{Cue, MergeFinding, Snapshot, Verdict};

const NS_PER_MS: f64 = 1_000_000.0;

fn overlap_ns(a: &Cue, b: &Cue) -> Option<u64> {
    let (a0, a1) = (a.start_ns?, a.end_ns?);
    let (b0, b1) = (b.start_ns?, b.end_ns?);
    let lo = a0.max(b0);
    let hi = a1.min(b1);
    hi.checked_sub(lo)
}

fn fmt_ts(ns: Option<u64>) -> String {
    match ns {
        Some(value) => {
            let ms = value / 1_000_000;
            format!(
                "{:02}:{:02}:{:02},{:03}",
                ms / 3_600_000,
                (ms % 3_600_000) / 60_000,
                (ms % 60_000) / 1000,
                ms % 1000
            )
        }
        None => "??".to_string(),
    }
}

fn cue_evidence(tag: char, cue: &Cue) -> Vec<String> {
    let loc = &cue.loc;
    let head = format!(
        "[{tag}] 记录#{} 原始行 {}-{}（{}）",
        loc.record_index, loc.line_start, loc.line_end, loc.snapshot_id
    );
    let time = format!(
        "[{tag}] 时间区间 {} --> {}",
        fmt_ts(cue.start_ns),
        fmt_ts(cue.end_ns)
    );
    let raw_trimmed = String::from_utf8_lossy(&cue.raw_record);
    let raw_preview: String = raw_trimmed.chars().take(48).collect();
    let mut out = vec![head, time, format!("[{tag}] 原文: {raw_preview}")];
    if cue.defects.is_empty() {
        out.push(format!("[{tag}] 结构检查: 无缺陷"));
    } else {
        for defect in &cue.defects {
            out.push(format!("[{tag}] 缺陷: {defect}"));
        }
        let tail: Vec<String> = cue
            .raw_record
            .iter()
            .rev()
            .take(4)
            .rev()
            .map(|b| format!("0x{b:02X}"))
            .collect();
        out.push(format!("[{tag}] 末尾字节证据: {}", tail.join(" ")));
    }
    out
}

/// 对两个快照执行合并判定，返回带证据的结论列表。
pub fn merge(snapshot_a: &Snapshot, snapshot_b: &Snapshot) -> Vec<MergeFinding> {
    let mut findings = Vec::new();

    for (b_index, b_cue) in snapshot_b.cues.iter().enumerate() {
        let finding = analyze_one(b_index, b_cue, snapshot_a);
        findings.push(finding);
    }
    findings
}

#[cfg(test)]
impl MergeFinding {
    fn overlap_evidence_ms(&self) -> f64 {
        for line in &self.evidence {
            if let Some(rest) = line.strip_prefix("[A] 候选记录") {
                let start = rest.find("重叠 ").map(|i| i + "重叠 ".len()).unwrap();
                let end = rest.find(" ms").unwrap();
                return rest[start..end].parse().unwrap();
            }
        }
        0.0
    }
}

fn analyze_one(b_index: usize, b_cue: &Cue, snapshot_a: &Snapshot) -> MergeFinding {
    let mut evidence = cue_evidence('B', b_cue);

    // 1. 来源自身损坏或截断：直接给出结论，证据已在 cue_evidence 中保留。
    if b_cue.end_ns.is_none() || b_cue.start_ns.is_none() {
        let truncated = b_cue
            .defects
            .iter()
            .any(|d| d.contains("截断") || d.contains("结束时间缺失"));
        let verdict = if truncated {
            Verdict::Truncated
        } else {
            Verdict::Malformed
        };
        return MergeFinding {
            index: b_index,
            reason: format!("B 侧记录#{} 来源不完整，无法参与时间轴合并", b_index),
            verdict,
            a_records: vec![],
            b_records: vec![b_index],
            evidence,
        };
    }

    // 2. 计算与 A 侧每条完整记录的重叠。
    let mut scored: Vec<(usize, f64, &Cue)> = Vec::new();
    for a_cue in &snapshot_a.cues {
        if a_cue.start_ns.is_none() || a_cue.end_ns.is_none() {
            continue;
        }
        if let Some(overlap) = overlap_ns(a_cue, b_cue) {
            if overlap > 0 {
                scored.push((a_cue.loc.record_index, overlap as f64 / NS_PER_MS, a_cue));
            }
        }
    }

    if scored.is_empty() {
        return MergeFinding {
            index: b_index,
            verdict: Verdict::Malformed,
            reason: format!("B 侧记录#{} 在 A 快照中找不到任何时间重叠", b_index),
            a_records: vec![],
            b_records: vec![b_index],
            evidence,
        };
    }

    let best = scored.iter().map(|(_, ms, _)| *ms).fold(f64::NAN, f64::max);
    let tied: Vec<&(usize, f64, &Cue)> = scored
        .iter()
        .filter(|(_, ms, _)| (*ms - best).abs() < 0.001)
        .collect();

    for (a_index, ms, a_cue) in &scored {
        evidence.push(format!(
            "[A] 候选记录#{a_index} 重叠 {ms:.1} ms（{} --> {}）",
            fmt_ts(a_cue.start_ns),
            fmt_ts(a_cue.end_ns)
        ));
        evidence.extend(cue_evidence('A', a_cue));
    }

    // 3. 边界情况：两个候选重叠度完全相等 → 无法自动判断。
    if tied.len() >= 2 {
        let tied_indexes: Vec<usize> = tied.iter().map(|(idx, _, _)| *idx).collect();
        evidence.push(format!(
            "[判定] 候选 {:?} 重叠时长均为 {:.1} ms，缺少打破平局的依据",
            tied_indexes, best
        ));
        return MergeFinding {
            index: b_index,
            verdict: Verdict::Ambiguous,
            reason: format!(
                "B 侧记录#{} 同时等权重叠 A 侧 {:?}，边界归属无法判断",
                b_index, tied_indexes
            ),
            a_records: tied_indexes,
            b_records: vec![b_index],
            evidence,
        };
    }

    let (best_a, best_ms, best_cue) = scored
        .iter()
        .copied()
        .max_by(|x, y| x.1.partial_cmp(&y.1).unwrap())
        .unwrap();

    // 4. 起止时间完全一致 → 已对齐；否则属于部分重叠。
    let exact = best_cue.start_ns == b_cue.start_ns && best_cue.end_ns == b_cue.end_ns;
    let verdict = if exact {
        Verdict::Aligned
    } else {
        Verdict::Overlap
    };
    evidence.push(format!(
        "[判定] 最大重叠 {best_ms:.1} ms，精确对齐 = {exact}"
    ));

    MergeFinding {
        index: b_index,
        verdict,
        reason: format!(
            "B 侧记录#{} 与 A 侧记录#{} 重叠 {:.1} ms",
            b_index, best_a, best_ms
        ),
        a_records: vec![best_a],
        b_records: vec![b_index],
        evidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_srt;

    fn snapshot(id: &str, raw: &[u8]) -> Snapshot {
        Snapshot {
            id: id.to_string(),
            label: id.to_string(),
            lang: "x".to_string(),
            raw_bytes: raw.to_vec(),
            sha256: String::new(),
            byte_len: raw.len(),
            line_count: 0,
            staged_path: std::path::PathBuf::from("/tmp/x.srt"),
            cues: parse_srt(id, raw),
        }
    }

    #[test]
    fn equal_overlap_boundary_is_ambiguous() {
        let a = snapshot(
            "a",
            concat!(
                "1\n00:00:04,000 --> 00:00:05,500\nA2\n\n",
                "2\n00:00:05,500 --> 00:00:07,000\nA3\n\n"
            )
            .as_bytes(),
        );
        let b = snapshot("b", b"1\n00:00:04,000 --> 00:00:07,000\nB-spans-both\n\n");
        let findings = merge(&a, &b);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].verdict, Verdict::Ambiguous);
        assert_eq!(findings[0].a_records, vec![0, 1]);
        assert_eq!(findings[0].overlap_evidence_ms(), 1500.0);
        assert!(findings[0]
            .evidence
            .iter()
            .any(|e| e.contains("无法判断") || e.contains("平局")));
    }
}
