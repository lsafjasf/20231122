use crate::model::{JournalLine, RawEvent};
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

/// 顺序写日志；`write_partial` 模拟崩溃导致的截断行。
pub struct JournalWriter {
    file: File,
    pub path: PathBuf,
    pub lines: usize,
}

impl JournalWriter {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            file: File::create(path)?,
            path: path.to_path_buf(),
            lines: 0,
        })
    }

    pub fn write_event(&mut self, ev: &RawEvent) -> std::io::Result<()> {
        let mut s = serde_json::to_string(ev).unwrap();
        s.push('\n');
        self.file.write_all(s.as_bytes())?;
        self.file.flush()?;
        self.lines += 1;
        Ok(())
    }

    /// 写入半行后停止，模拟记录被截断（崩溃/掉电）。
    pub fn write_partial(&mut self, ev: &RawEvent) -> std::io::Result<()> {
        let s = serde_json::to_string(ev).unwrap();
        let cut = s.len() * 2 / 3;
        self.file.write_all(&s.as_bytes()[..cut])?;
        self.file.flush()?;
        self.lines += 1;
        Ok(())
    }
}

/// 读取日志：完整行解析为事件；无法解析的行（截断）保留原文作为证据。
pub fn read_journal(path: &Path) -> std::io::Result<Vec<JournalLine>> {
    let f = File::open(path)?;
    let mut out = Vec::new();
    for (i, line) in BufReader::new(f).lines().enumerate() {
        let raw = line?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (event, parse_error) = match serde_json::from_str::<RawEvent>(trimmed) {
            Ok(ev) => (Some(ev), None),
            Err(e) => (None, Some(e.to_string())),
        };
        out.push(JournalLine {
            line_no: i + 1,
            raw: raw.clone(),
            event,
            parse_error,
        });
    }
    Ok(out)
}

/// 从若干日志中提取每通道最后一条完整 checkpoint，供恢复重放使用。
pub fn last_checkpoints(lines: &[JournalLine]) -> std::collections::BTreeMap<u32, (u64, u64)> {
    let mut map = std::collections::BTreeMap::new();
    for l in lines {
        if let Some(RawEvent::Checkpoint {
            channel,
            last_seq,
            wall_ms,
        }) = &l.event
        {
            map.insert(*channel, (*last_seq, *wall_ms));
        }
    }
    map
}
