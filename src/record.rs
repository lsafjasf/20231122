//! 安全框检测记录：JSONL 格式，兼容旧版 v1 与新版 v2。
//!
//! v1（旧格式）：每行一条 entry，无头部、无参数、无校验。
//! v2（新格式）：首行 header（含处理参数与 params_hash），随后逐条 entry
//! （带 seq / pts / frame / track / metric / params_hash），末尾 eof 行携带
//! 输出波形（metric 序列）的 CRC32，把时间点、帧、参数和波形绑定在一起。
//! 记录可能被截断（崩溃/掉电）；recover 保留原始截断文件作为证据，
//! 截断到最近一条完整行，并追加 recovered 标记行。
use crate::crc::crc32;
use crate::params::SafeFrameParams;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub seq: u64,
    pub pts_ms: i64,
    pub frame: u64,
    pub track: String,
    /// 输出波形的一个采样：安全框外活跃像素占比
    pub metric: f64,
    pub violation: bool,
    #[serde(default)]
    pub params_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderV2 {
    pub v: u32,
    #[serde(rename = "type")]
    pub kind: String,
    pub tool: String,
    pub media: String,
    pub params: SafeFrameParams,
    pub params_hash: String,
    pub created_unix: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryMark {
    pub dropped_bytes: u64,
    pub resumed_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    V1,
    V2,
}

#[derive(Debug)]
pub struct Record {
    pub format: Format,
    pub header: Option<HeaderV2>,
    pub entries: Vec<Entry>,
    /// 恢复信息（文件曾被截断并修复）
    pub recovered: Option<RecoveryMark>,
    /// eof 行存在且波形 CRC 校验通过
    pub wave_crc_ok: Option<bool>,
    /// 加载时发现的尾部垃圾字节数（未恢复时 >0）
    pub tail_garbage_bytes: u64,
}

impl Record {
    /// 第一个错误状态：第一条 violation entry。
    /// 复核时只给总分没有意义，必须能直接指出这一帧。
    pub fn first_error(&self) -> Option<&Entry> {
        self.entries.iter().find(|e| e.violation)
    }

    /// 输出波形：metric 序列
    pub fn waveform(&self) -> Vec<f64> {
        self.entries.iter().map(|e| e.metric).collect()
    }
}

fn wave_crc(entries: &[Entry]) -> u32 {
    let mut buf = Vec::with_capacity(entries.len() * 4);
    for e in entries {
        buf.extend_from_slice(&(e.metric as f32).to_le_bytes());
    }
    crc32(&buf)
}

/// v2 写入器：逐行落盘并 flush，模拟可崩溃的长任务。
pub struct WriterV2 {
    file: fs::File,
    entries: Vec<Entry>,
    params_hash: String,
}

impl WriterV2 {
    pub fn create(path: &Path, media: &str, params: &SafeFrameParams) -> std::io::Result<Self> {
        let mut file = fs::File::create(path)?;
        let header = HeaderV2 {
            v: 2,
            kind: "header".into(),
            tool: "safe-frame-audit".into(),
            media: media.into(),
            params: params.clone(),
            params_hash: params.hash(),
            created_unix: unix_now(),
        };
        writeln!(file, "{}", serde_json::to_string(&header).unwrap())?;
        file.flush()?;
        Ok(Self {
            file,
            entries: Vec::new(),
            params_hash: params.hash(),
        })
    }

    pub fn push(
        &mut self,
        pts_ms: i64,
        frame: u64,
        track: &str,
        metric: f64,
        violation: bool,
    ) -> std::io::Result<()> {
        let seq = self.entries.len() as u64;
        let e = Entry {
            seq,
            pts_ms,
            frame,
            track: track.into(),
            metric,
            violation,
            params_hash: self.params_hash.clone(),
        };
        let line = serde_json::json!({
            "v": 2, "type": "entry", "seq": e.seq, "pts_ms": e.pts_ms,
            "frame": e.frame, "track": e.track, "metric": e.metric,
            "violation": e.violation, "params_hash": e.params_hash,
        });
        writeln!(self.file, "{line}")?;
        self.file.flush()?;
        self.entries.push(e);
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        let eof = serde_json::json!({
            "v": 2, "type": "eof", "entries": self.entries.len(),
            "wave_crc32": format!("{:08x}", wave_crc(&self.entries)),
        });
        writeln!(self.file, "{eof}")?;
        self.file.flush()
    }
}

/// 旧格式 v1 写入（用于生成兼容测试夹具）。
pub fn write_v1(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let mut f = fs::File::create(path)?;
    for e in entries {
        writeln!(
            f,
            "{}",
            serde_json::json!({
                "v": 1, "pts_ms": e.pts_ms, "frame": e.frame, "track": e.track,
                "metric": e.metric, "violation": e.violation,
            })
        )?;
    }
    Ok(())
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 加载记录，自动识别 v1/v2。容忍尾部垃圾（截断），
/// 在第一个无法解析的行停止并记录垃圾字节数。
pub fn load(path: &Path) -> std::io::Result<Record> {
    let raw = fs::read(path)?;
    Ok(parse(&raw))
}

pub fn parse(raw: &[u8]) -> Record {
    let mut format = None;
    let mut header = None;
    let mut entries = Vec::new();
    let mut recovered = None;
    let mut wave_crc_ok = None;
    let mut tail_garbage_bytes = 0u64;
    let mut offset = 0usize;

    for line in raw.split(|&b| b == b'\n') {
        let len_with_nl = line.len() + 1;
        let text = String::from_utf8_lossy(line);
        let text = text.trim();
        if text.is_empty() {
            offset += len_with_nl;
            continue;
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
            // 截断的垃圾尾：记录剩余字节后停止
            tail_garbage_bytes = (raw.len() - offset) as u64;
            break;
        };
        offset += len_with_nl;
        let v = val.get("v").and_then(|x| x.as_u64()).unwrap_or(1) as u32;
        let kind = val.get("type").and_then(|x| x.as_str()).unwrap_or("");
        if format.is_none() {
            format = Some(if v >= 2 { Format::V2 } else { Format::V1 });
        }
        match (v, kind) {
            (2, "header") => {
                if let Ok(h) = serde_json::from_value::<HeaderV2>(val.clone()) {
                    header = Some(h);
                }
            }
            (2, "entry") => {
                if let Some(e) = parse_entry(&val, true) {
                    entries.push(e);
                }
            }
            (2, "eof") => {
                let expect = val
                    .get("wave_crc32")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                if let Some(expect) = expect {
                    wave_crc_ok = Some(format!("{:08x}", wave_crc(&entries)) == expect);
                }
            }
            (2, "recovered") => {
                recovered = Some(RecoveryMark {
                    dropped_bytes: val
                        .get("dropped_bytes")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0),
                    resumed_seq: val.get("resumed_seq").and_then(|x| x.as_u64()).unwrap_or(0),
                });
            }
            (1, "") => {
                if val.get("recovered").and_then(|x| x.as_bool()) == Some(true) {
                    recovered = Some(RecoveryMark {
                        dropped_bytes: val
                            .get("dropped_bytes")
                            .and_then(|x| x.as_u64())
                            .unwrap_or(0),
                        resumed_seq: entries.len() as u64,
                    });
                } else if let Some(e) = parse_entry(&val, false) {
                    entries.push(e);
                }
            }
            _ => {} // 未知行：宽容跳过，保证旧工具也能读
        }
    }

    Record {
        format: format.unwrap_or(Format::V1),
        header,
        entries,
        recovered,
        wave_crc_ok,
        tail_garbage_bytes,
    }
}

fn parse_entry(val: &serde_json::Value, has_seq: bool) -> Option<Entry> {
    let g = |k: &str| val.get(k);
    Some(Entry {
        seq: if has_seq {
            g("seq")?.as_u64()?
        } else {
            g("frame")?.as_u64()?
        },
        pts_ms: g("pts_ms")?.as_i64()?,
        frame: g("frame")?.as_u64()?,
        track: g("track")
            .and_then(|x| x.as_str())
            .unwrap_or("video0")
            .to_string(),
        metric: g("metric")?.as_f64()?,
        violation: g("violation")?.as_bool()?,
        params_hash: g("params_hash")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

/// 恢复被截断的记录。
/// 1) 原始截断文件复制为 `<name>.truncated.bak`（关键证据，不覆盖）；
/// 2) 截断到最近一条完整行；
/// 3) 追加 recovered 标记行（v1/v2 各自语法）。
pub fn recover(path: &Path) -> std::io::Result<RecoveryMark> {
    let raw = fs::read(path)?;
    let parsed = parse(&raw);
    if parsed.tail_garbage_bytes == 0 {
        return Ok(RecoveryMark {
            dropped_bytes: 0,
            resumed_seq: parsed.entries.len() as u64,
        });
    }
    let keep = raw.len() - parsed.tail_garbage_bytes as usize;

    let mut bak = path.as_os_str().to_os_string();
    bak.push(".truncated.bak");
    fs::write(Path::new(&bak), &raw)?; // 证据保留

    let mut fixed = raw[..keep].to_vec();
    if !fixed.ends_with(b"\n") && !fixed.is_empty() {
        fixed.push(b'\n');
    }
    let mark = RecoveryMark {
        dropped_bytes: parsed.tail_garbage_bytes,
        resumed_seq: parsed.entries.len() as u64,
    };
    let line = match parsed.format {
        Format::V2 => serde_json::json!({
            "v": 2, "type": "recovered",
            "dropped_bytes": mark.dropped_bytes, "resumed_seq": mark.resumed_seq,
        }),
        Format::V1 => serde_json::json!({
            "v": 1, "recovered": true, "dropped_bytes": mark.dropped_bytes,
        }),
    };
    fixed.extend_from_slice(line.to_string().as_bytes());
    fixed.push(b'\n');
    fs::write(path, fixed)?;
    Ok(mark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("sfar-test-{}", std::process::id()));
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample_entries() -> Vec<Entry> {
        (0..10)
            .map(|i| Entry {
                seq: i,
                pts_ms: i as i64 * 100,
                frame: i,
                track: "video0".into(),
                metric: if i >= 6 { 0.05 } else { 0.0 },
                violation: i >= 6,
                params_hash: "abc".into(),
            })
            .collect()
    }

    #[test]
    fn v2_roundtrip_and_first_error() {
        let dir = tmpdir();
        let p = dir.join("roundtrip.sfar2.jsonl");
        let params = SafeFrameParams::default();
        let mut w = WriterV2::create(&p, "test.mkv", &params).unwrap();
        for e in sample_entries() {
            w.push(e.pts_ms, e.frame, &e.track, e.metric, e.violation)
                .unwrap();
        }
        w.finish().unwrap();

        let rec = load(&p).unwrap();
        assert_eq!(rec.format, Format::V2);
        assert_eq!(rec.entries.len(), 10);
        assert_eq!(rec.wave_crc_ok, Some(true));
        assert_eq!(rec.tail_garbage_bytes, 0);
        let first = rec.first_error().unwrap();
        assert_eq!(first.frame, 6);
        assert_eq!(first.pts_ms, 600);
        assert_eq!(rec.header.unwrap().params_hash, params.hash());
    }

    #[test]
    fn truncated_v2_recovers_with_evidence() {
        let dir = tmpdir();
        let p = dir.join("trunc.sfar2.jsonl");
        let params = SafeFrameParams::default();
        let mut w = WriterV2::create(&p, "test.mkv", &params).unwrap();
        for e in sample_entries() {
            w.push(e.pts_ms, e.frame, &e.track, e.metric, e.violation)
                .unwrap();
        }
        w.finish().unwrap();

        // 模拟崩溃：文件被截断到行中间
        let raw = fs::read(&p).unwrap();
        let cut = raw.len() - 17;
        fs::write(&p, &raw[..cut]).unwrap();

        let mark = recover(&p).unwrap();
        assert!(mark.dropped_bytes > 0);

        // 证据：原始截断文件被保留
        let bak = p.parent().unwrap().join(format!(
            "{}.truncated.bak",
            p.file_name().unwrap().to_string_lossy()
        ));
        assert!(bak.exists());
        assert_eq!(fs::read(&bak).unwrap(), raw[..cut].to_vec());

        let rec = load(&p).unwrap();
        assert_eq!(rec.tail_garbage_bytes, 0);
        assert!(rec.recovered.is_some());
        // eof 行被截掉，但 entry 完整保留，首错仍可定位
        assert_eq!(rec.entries.len(), 10);
        assert_eq!(rec.first_error().unwrap().frame, 6);
    }

    #[test]
    fn v1_legacy_compat() {
        let dir = tmpdir();
        let p = dir.join("legacy.sfar1.jsonl");
        write_v1(&p, &sample_entries()).unwrap();
        let rec = load(&p).unwrap();
        assert_eq!(rec.format, Format::V1);
        assert_eq!(rec.entries.len(), 10);
        assert_eq!(rec.first_error().unwrap().frame, 6);
        assert!(rec.header.is_none());
    }

    #[test]
    fn v1_truncated_recovers() {
        let dir = tmpdir();
        let p = dir.join("legacy-trunc.sfar1.jsonl");
        write_v1(&p, &sample_entries()).unwrap();
        let raw = fs::read(&p).unwrap();
        fs::write(&p, &raw[..raw.len() - 31]).unwrap();
        let mark = recover(&p).unwrap();
        assert!(mark.dropped_bytes > 0);
        let rec = load(&p).unwrap();
        assert_eq!(rec.format, Format::V1);
        assert_eq!(rec.entries.len(), 9);
        assert!(rec.recovered.is_some());
    }
}
