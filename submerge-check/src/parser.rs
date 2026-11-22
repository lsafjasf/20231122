//! 容错 SRT 解析器：按原始字节切分记录，任何单条记录失败都不影响其余记录。

use crate::model::{Cue, SourceLoc};

struct RawLine {
    number: usize,
    text: String,
}

/// 将原始字节按行切分（`\n`，兼容 `\r\n`），非法 UTF-8 使用替换字符兜底，
/// 但记录级原始字节仍完整保留在 `Cue::raw_record` 中。
fn split_lines(raw: &[u8]) -> Vec<RawLine> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut number = 1usize;
    for (idx, byte) in raw.iter().enumerate() {
        if *byte == b'\n' {
            let mut end = idx;
            if end > start && raw[end - 1] == b'\r' {
                end -= 1;
            }
            let text = String::from_utf8_lossy(&raw[start..end]).into_owned();
            lines.push(RawLine { number, text });
            number += 1;
            start = idx + 1;
        }
    }
    if start < raw.len() {
        let text = String::from_utf8_lossy(&raw[start..]).into_owned();
        lines.push(RawLine { number, text });
    }
    lines
}

/// 解析 `HH:MM:SS,mmm` 为纳秒；失败返回 `None` 并附带原因。
fn parse_timestamp(token: &str) -> Result<u64, String> {
    let parts: Vec<&str> = token.trim().split(':').collect();
    if parts.len() != 3 {
        return Err(format!("时间戳段数错误: {token:?}"));
    }
    let hour: u64 = parts[0]
        .parse()
        .map_err(|_| format!("小时非法: {token:?}"))?;
    let minute: u64 = parts[1]
        .parse()
        .map_err(|_| format!("分钟非法: {token:?}"))?;
    let (sec_str, ms_str) = parts[2]
        .split_once(',')
        .ok_or_else(|| format!("缺少毫秒分隔符: {token:?}"))?;
    let second: u64 = sec_str.parse().map_err(|_| format!("秒非法: {token:?}"))?;
    let milli: u64 = ms_str.parse().map_err(|_| format!("毫秒非法: {token:?}"))?;
    Ok(((hour * 3600 + minute * 60 + second) * 1000 + milli) * 1_000_000)
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// 解析整份快照；返回的每条 `Cue` 都带原始定位与字节证据。
pub fn parse_srt(snapshot_id: &str, raw: &[u8]) -> Vec<Cue> {
    let lines = split_lines(raw);
    let mut cues = Vec::new();
    let mut block: Vec<&RawLine> = Vec::new();
    let mut record_index = 0usize;

    let flush = |block: &mut Vec<&RawLine>, cues: &mut Vec<Cue>, record_index: &mut usize| {
        if block.is_empty() {
            return;
        }
        let first = block.first().unwrap();
        let last = block.last().unwrap();
        let raw_start = line_offset(raw, first.number);
        let raw_end = line_offset(raw, last.number + 1).min(raw.len());
        let cue = parse_record(
            snapshot_id,
            *record_index,
            block,
            raw[raw_start..raw_end].to_vec(),
        );
        cues.push(cue);
        *record_index += 1;
        block.clear();
    };

    for line in &lines {
        if is_blank(&line.text) {
            flush(&mut block, &mut cues, &mut record_index);
        } else {
            block.push(line);
        }
    }
    flush(&mut block, &mut cues, &mut record_index);
    cues
}

/// 计算第 `line_no` 行（1 起始）首字节在原始输入中的偏移；
/// 超出范围时返回输入长度。
fn line_offset(raw: &[u8], line_no: usize) -> usize {
    let mut current = 1usize;
    if line_no <= 1 {
        return 0;
    }
    for (idx, byte) in raw.iter().enumerate() {
        if *byte == b'\n' {
            current += 1;
            if current == line_no {
                return idx + 1;
            }
        }
    }
    raw.len()
}

fn parse_record(
    snapshot_id: &str,
    record_index: usize,
    block: &[&RawLine],
    raw_record: Vec<u8>,
) -> Cue {
    let mut defects = Vec::new();
    let first = block[0];
    let last = block[block.len() - 1];
    let loc = SourceLoc {
        snapshot_id: snapshot_id.to_string(),
        line_start: first.number,
        line_end: last.number,
        record_index,
    };

    if std::str::from_utf8(&raw_record).is_err() {
        defects.push("记录包含非法 UTF-8 字节（疑似多字节字符被截断）".to_string());
    }

    let mut cursor = 0usize;
    let seq = if first.text.trim().bytes().all(|b| b.is_ascii_digit()) {
        let parsed = first.text.trim().parse::<u32>().ok();
        if parsed.is_none() {
            defects.push(format!("序号非法: {:?}", first.text));
        }
        cursor = 1;
        parsed
    } else {
        defects.push(format!("首行不是数字序号: {:?}", first.text));
        None
    };

    let mut start_ns = None;
    let mut end_ns = None;
    let time_line = block
        .get(cursor)
        .map(|l| l.text.clone())
        .unwrap_or_default();
    if time_line.is_empty() {
        defects.push("记录在序号后被截断，缺少时间轴行".to_string());
    } else if let Some((left, right)) = time_line.split_once("-->") {
        match parse_timestamp(left) {
            Ok(ns) => start_ns = Some(ns),
            Err(err) => defects.push(format!("开始时间 {err}")),
        }
        let right = right.trim();
        if right.is_empty() {
            defects.push("结束时间缺失：`-->` 后无时间戳（记录被截断）".to_string());
        } else {
            match parse_timestamp(right) {
                Ok(ns) => end_ns = Some(ns),
                Err(err) => defects.push(format!("结束时间 {err}")),
            }
        }
        cursor += 1;
    } else {
        defects.push(format!("时间轴行缺少 `-->` 结构: {time_line:?}"));
        cursor += 1;
    }

    let text_lines: Vec<String> = block[cursor.min(block.len())..]
        .iter()
        .map(|l| l.text.clone())
        .collect();
    if text_lines.join("\n").trim().is_empty() {
        defects.push("字幕正文为空".to_string());
    }

    Cue {
        seq,
        start_ns,
        end_ns,
        text: text_lines.join("\n"),
        loc,
        raw_record,
        defects,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_well_formed_record() {
        let raw = "1\n00:00:01,500 --> 00:00:02,000\nhello\n\n".as_bytes();
        let cues = parse_srt("snapshot_a", raw);
        assert_eq!(cues.len(), 1);
        let cue = &cues[0];
        assert_eq!(cue.seq, Some(1));
        assert_eq!(cue.start_ns, Some(1_500_000_000));
        assert_eq!(cue.end_ns, Some(2_000_000_000));
        assert_eq!(cue.text, "hello");
        assert!(cue.defects.is_empty());
        assert_eq!(cue.loc.line_start, 1);
        assert_eq!(cue.loc.line_end, 3);
    }

    #[test]
    fn truncated_record_keeps_bytes_and_flags_defect() {
        let mut raw = b"5\n00:00:12,000 -->\n\xe6\x8d".to_vec();
        raw.extend_from_slice(&[0xE6, 0x8D]);
        let cues = parse_srt("snapshot_b", &raw);
        assert_eq!(cues.len(), 1);
        let cue = &cues[0];
        assert_eq!(cue.start_ns, Some(12_000_000_000));
        assert_eq!(cue.end_ns, None);
        assert!(cue.defects.iter().any(|d| d.contains("结束时间缺失")));
        assert!(cue.defects.iter().any(|d| d.contains("非法 UTF-8")));
        assert!(cue.raw_record.ends_with(&[0xE6, 0x8D]));
        assert_eq!(cue.loc.line_start, 1);
        assert_eq!(cue.loc.line_end, 3);
    }

    #[test]
    fn malformed_record_does_not_block_next_record() {
        let raw = concat!(
            "1\n",
            "00:00:01,000 - 00:00:02,000\n",
            "broken\n",
            "\n",
            "2\n",
            "00:00:03,000 --> 00:00:04,000\n",
            "fine\n",
            "\n",
        )
        .as_bytes();
        let cues = parse_srt("snapshot_b", raw);
        assert_eq!(cues.len(), 2);
        assert!(cues[0].defects.iter().any(|d| d.contains("`-->`")));
        assert!(cues[1].defects.is_empty());
        assert_eq!(cues[1].loc.record_index, 1);
    }
}
