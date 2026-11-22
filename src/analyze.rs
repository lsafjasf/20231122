use crate::journal::read_journal;
use crate::model::*;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DRIFT_TOLERANCE_MS: i64 = 40;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub steps: Vec<ProcessingStep>,
    pub findings: Vec<Finding>,
    /// 第一个错误状态（按时间排序），而非总分。
    pub first_error: Option<usize>,
    pub indeterminate_count: usize,
    pub error_count: usize,
    pub channels: Vec<ChannelSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSummary {
    pub channel: u32,
    pub frames: u64,
    pub verdict: Verdict,
}

struct Ctx {
    steps: Vec<ProcessingStep>,
    findings: Vec<Finding>,
}

impl Ctx {
    fn add_step(&mut self, name: &str, detail: String) -> usize {
        let index = self.steps.len();
        self.steps.push(ProcessingStep {
            index,
            name: name.to_string(),
            detail,
        });
        index
    }
    fn add_finding(
        &mut self,
        verdict: Verdict,
        kind: &str,
        channel: Option<u32>,
        at_ms: Option<u64>,
        summary: String,
        step_index: usize,
        evidence: Vec<EvidenceRef>,
    ) -> usize {
        let id = self.findings.len();
        self.findings.push(Finding {
            id,
            verdict,
            kind: kind.to_string(),
            channel,
            at_ms,
            summary,
            step_index,
            evidence,
        });
        id
    }
}

fn ev_ref(file: &str, line: &JournalLine, note: &str) -> EvidenceRef {
    EvidenceRef {
        source_file: file.to_string(),
        line_no: line.line_no,
        raw_excerpt: line.raw.chars().take(160).collect(),
        note: note.to_string(),
    }
}

/// 分析入口：inputs 为按时间顺序的日志文件（原始记录 + 恢复重放）。
pub fn analyze(inputs: &[&Path]) -> std::io::Result<Report> {
    let mut ctx = Ctx {
        steps: Vec::new(),
        findings: Vec::new(),
    };

    // 步骤 0：加载原始输入
    let s0 = ctx.add_step(
        "load-journals",
        format!("加载 {} 个日志文件，逐行解析 JSONL", inputs.len()),
    );
    let mut files: Vec<(String, Vec<JournalLine>)> = Vec::new();
    for p in inputs {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        files.push((name, read_journal(p)?));
    }
    let total: usize = files.iter().map(|f| f.1.len()).sum();
    ctx.add_finding(
        Verdict::Ok,
        "input-loaded",
        None,
        None,
        format!("载入 {} 行原始记录", total),
        s0,
        files
            .iter()
            .flat_map(|(n, ls)| ls.first().map(|l| ev_ref(n, l, "文件首行")))
            .collect(),
    );

    // 步骤 1：截断扫描 —— 解析失败的行即被截断的记录
    let s1 = ctx.add_step(
        "truncation-scan",
        "扫描解析失败的行，定位被截断的记录".to_string(),
    );
    let mut truncations: Vec<(String, JournalLine)> = Vec::new();
    for (name, lines) in &files {
        for l in lines {
            if l.event.is_none() {
                truncations.push((name.clone(), l.clone()));
            }
        }
    }
    for (name, line) in &truncations {
        // 截断时刻 ≈ 该文件中截断行之前最后一帧的墙钟时间
        let at_ms = files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, lines)| {
                lines
                    .iter()
                    .filter(|l| l.line_no < line.line_no)
                    .filter_map(|l| match &l.event {
                        Some(RawEvent::Frame { wall_ms, .. }) => Some(*wall_ms),
                        _ => None,
                    })
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        ctx.add_finding(
            Verdict::Error,
            "record-truncated",
            None,
            Some(at_ms),
            format!(
                "记录被截断：{} 第 {} 行不是完整事件（{}）",
                name,
                line.line_no,
                line.parse_error.clone().unwrap_or_default()
            ),
            s1,
            vec![ev_ref(name, line, "截断的半行，原样保留为关键证据")],
        );
    }

    // 步骤 2：恢复重放拼接 —— 截断点与重放起点之间为无法判断的边界
    let s2 = ctx.add_step(
        "recovery-splice",
        "对比截断尾部与重放头部，划定无法判断的边界窗口".to_string(),
    );
    if !truncations.is_empty() && files.len() > 1 {
        let (tfile, tline) = &truncations[0];
        // 截断文件中每通道最后一条完整帧
        let mut last_good: std::collections::BTreeMap<u32, (usize, JournalLine, String)> =
            std::collections::BTreeMap::new();
        for (name, lines) in &files {
            for l in lines {
                if let Some(RawEvent::Frame { channel, seq, .. }) = &l.event {
                    if name == tfile {
                        last_good.insert(*channel, (*seq as usize, l.clone(), name.clone()));
                    }
                }
            }
        }
        // 重放文件中的 Recovered 标记与首帧
        let replay = &files[1];
        let recovered = replay.1.iter().find(|l| {
            matches!(l.event, Some(RawEvent::Recovered { .. }))
        });
        let first_replay_frame = replay
            .1
            .iter()
            .find(|l| matches!(l.event, Some(RawEvent::Frame { .. })));
        for (ch, (seq, line, name)) in &last_good {
            let mut evidence = vec![
                ev_ref(name, line, &format!("通道 {} 截断前最后完整帧 seq={}", ch, seq)),
                ev_ref(tfile, tline, "截断点：此后的记录丢失"),
            ];
            if let Some(r) = recovered {
                evidence.push(ev_ref(&replay.0, r, "恢复重放起点标记"));
            }
            if let Some(f) = first_replay_frame {
                evidence.push(ev_ref(&replay.0, f, "重放后的首帧"));
            }
            ctx.add_finding(
                Verdict::Indeterminate,
                "boundary-unknown",
                Some(*ch),
                None,
                format!(
                    "通道 {}：截断点与恢复重放之间是否存在丢失帧无法判断，边界证据已保留",
                    ch
                ),
                s2,
                evidence,
            );
        }
    }

    // 步骤 3：帧序列连续性 —— 同一记录区段内 seq 跳变即丢帧错误
    let s3 = ctx.add_step(
        "sequence-check",
        "按通道检查帧序号连续性，跳变处判定丢帧".to_string(),
    );
    // 帧序列与漂移必须在同一记录区段（同一文件）内检查；
    // 跨文件的连续性由 recovery-splice 步骤负责。
    let mut frames: Vec<(u64, u32, u64, &str, &JournalLine)> = Vec::new();
    for (name, lines) in &files {
        let mut file_frames: Vec<(u64, u32, u64, &str, &JournalLine)> = Vec::new();
        for l in lines {
            if let Some(RawEvent::Frame {
                channel,
                seq,
                wall_ms,
                ..
            }) = &l.event
            {
                file_frames.push((*wall_ms, *channel, *seq, name, l));
            }
        }
        file_frames.sort_by_key(|f| f.0);
        let mut prev: std::collections::BTreeMap<u32, (u64, u64, &JournalLine)> =
            std::collections::BTreeMap::new();
        for (wall, ch, seq, name, line) in &file_frames {
            if let Some((pseq, _pwall, pline)) = prev.get(ch) {
                let expected = pseq + 1;
                if *seq > expected {
                    ctx.add_finding(
                        Verdict::Error,
                        "frame-lost",
                        Some(*ch),
                        Some(*wall),
                        format!(
                            "通道 {} 丢帧：seq 从 {} 跳到 {}，丢失 {} 帧",
                            ch,
                            pseq,
                            seq,
                            seq - expected
                        ),
                        s3,
                        vec![
                            ev_ref(name, pline, "跳变前最后一帧"),
                            ev_ref(name, line, "跳变后第一帧"),
                        ],
                    );
                }
            }
            prev.insert(*ch, (*seq, *wall, line));
        }
        frames.extend(file_frames);
    }
    frames.sort_by_key(|f| f.0);

    // 步骤 4：同步漂移 —— 实际帧间隔相对 tick 的偏差超阈值即错误
    let s4 = ctx.add_step(
        "drift-check",
        format!("逐帧计算漂移，阈值 ±{}ms", DRIFT_TOLERANCE_MS),
    );
    let mut tick_ms: u64 = 33;
    for (_, lines) in &files {
        for l in lines {
            if let Some(RawEvent::SessionStart { tick_ms: t, .. }) = &l.event {
                tick_ms = *t;
            }
        }
    }
    // 漂移同样按文件分段检查
    for (name, lines) in &files {
        let mut file_frames: Vec<(u64, u32, &JournalLine)> = Vec::new();
        for l in lines {
            if let Some(RawEvent::Frame {
                channel, wall_ms, ..
            }) = &l.event
            {
                file_frames.push((*wall_ms, *channel, l));
            }
        }
        file_frames.sort_by_key(|f| f.0);
        let mut last_wall: std::collections::BTreeMap<u32, (u64, &JournalLine)> =
            std::collections::BTreeMap::new();
        for (wall, ch, line) in &file_frames {
            if let Some((pw, pline)) = last_wall.get(ch) {
                let delta = *wall as i64 - *pw as i64;
                let drift = delta - tick_ms as i64;
                if drift.abs() > DRIFT_TOLERANCE_MS {
                    ctx.add_finding(
                        Verdict::Error,
                        "drift-exceeded",
                        Some(*ch),
                        Some(*wall),
                        format!(
                            "通道 {} 同步漂移 {}ms（帧间隔 {}ms，期望 {}ms）",
                            ch, drift, delta, tick_ms
                        ),
                        s4,
                        vec![
                            ev_ref(name, pline, "前一帧"),
                            ev_ref(name, line, "漂移发生帧"),
                        ],
                    );
                }
            }
            last_wall.insert(*ch, (*wall, line));
        }
    }

    // 步骤 5：首个错误状态定位 —— 按 (时间, 结论 id) 排序取最早错误
    let s5 = ctx.add_step(
        "first-error",
        "按时间排序全部错误结论，定位第一个错误状态".to_string(),
    );
    let mut errors: Vec<&Finding> = ctx
        .findings
        .iter()
        .filter(|f| f.verdict == Verdict::Error)
        .collect();
    errors.sort_by_key(|f| (f.at_ms.unwrap_or(0), f.id));
    let first_error = errors.first().map(|f| f.id);
    if let Some(fe) = first_error {
        let f = &ctx.findings[fe];
        let (ch, at, kind) = (f.channel, f.at_ms, f.kind.clone());
        let ev = f.evidence.clone();
        ctx.add_finding(
            Verdict::Error,
            "first-error",
            ch,
            at,
            format!(
                "第一个错误状态：通道 {} @ {}ms（{}）",
                ch.map(|c| c.to_string()).unwrap_or("-".into()),
                at.map(|t| t.to_string()).unwrap_or("-".into()),
                kind
            ),
            s5,
            ev,
        );
    }

    // 通道汇总
    let mut channels: Vec<ChannelSummary> = Vec::new();
    let mut by_ch: std::collections::BTreeMap<u32, u64> = std::collections::BTreeMap::new();
    for (_, ch, _, _, _) in &frames {
        *by_ch.entry(*ch).or_default() += 1;
    }
    for (ch, count) in by_ch {
        let mut v = Verdict::Ok;
        for f in &ctx.findings {
            if f.channel == Some(ch) && f.verdict > v && f.kind != "first-error" {
                v = f.verdict;
            }
        }
        channels.push(ChannelSummary {
            channel: ch,
            frames: count,
            verdict: v,
        });
    }

    let error_count = ctx
        .findings
        .iter()
        .filter(|f| f.verdict == Verdict::Error && f.kind != "first-error")
        .count();
    let indeterminate_count = ctx
        .findings
        .iter()
        .filter(|f| f.verdict == Verdict::Indeterminate)
        .count();

    Ok(Report {
        steps: ctx.steps,
        findings: ctx.findings,
        first_error,
        indeterminate_count,
        error_count,
        channels,
    })
}
