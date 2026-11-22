use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::model::{DuplicateRecord, ParsedEvent, ParsedLog, RawEvent};

#[derive(Debug, Clone)]
pub struct EventSource {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct EventError {
    pub code: &'static str,
    pub message: String,
    pub raw_offset: Option<u64>,
}

impl EventError {
    fn io(path: &Path, error: std::io::Error) -> Self {
        Self {
            code: "E_EVENT_IO",
            message: format!("cannot read {}: {error}", path.display()),
            raw_offset: None,
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

pub fn load(
    path: &Path,
    max_bytes: u64,
    max_events: usize,
) -> Result<(EventSource, ParsedLog), EventError> {
    let metadata = fs::metadata(path).map_err(|error| EventError::io(path, error))?;
    if !metadata.is_file() {
        return Err(EventError {
            code: "E_EVENT_NOT_FILE",
            message: format!("{} is not a regular file", path.display()),
            raw_offset: None,
        });
    }
    if metadata.len() > max_bytes {
        return Err(EventError {
            code: "E_EVENT_BYTES_LIMIT",
            message: format!(
                "event input is {} bytes, exceeding configured limit {}",
                metadata.len(),
                max_bytes
            ),
            raw_offset: Some(max_bytes),
        });
    }
    let bytes = fs::read(path).map_err(|error| EventError::io(path, error))?;
    let source = EventSource {
        bytes: metadata.len(),
        sha256: sha256_hex(&bytes),
    };
    let log = parse(&bytes, max_events)?;
    Ok((source, log))
}

fn parse(bytes: &[u8], max_events: usize) -> Result<ParsedLog, EventError> {
    let mut events = Vec::new();
    let mut records_parsed = 0usize;
    let mut bad_json_records = 0usize;
    let mut offset = 0usize;
    let mut previous_raw_ts = None;
    let mut previous_raw_offset = None;

    while offset < bytes.len() {
        let line_end = bytes[offset..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |position| offset + position + 1);
        let raw = &bytes[offset..line_end];
        let body = raw.strip_suffix(b"\n").unwrap_or(raw);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        let raw_offset = offset as u64;
        let raw_len = raw.len() as u64;
        offset = line_end;

        if body.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if records_parsed >= max_events {
            return Err(EventError {
                code: "E_EVENT_COUNT_LIMIT",
                message: format!("event count exceeds configured limit {max_events}"),
                raw_offset: Some(raw_offset),
            });
        }
        records_parsed += 1;

        let event: RawEvent = match serde_json::from_slice(body) {
            Ok(event) => event,
            Err(_) => {
                bad_json_records += 1;
                continue;
            }
        };
        if !event.x.is_finite() || !event.y.is_finite() {
            bad_json_records += 1;
            continue;
        }

        let raw_order_inversion = previous_raw_ts.is_some_and(|previous| event.ts_ns < previous);
        events.push(ParsedEvent {
            seq: event.seq,
            ts_ns: event.ts_ns,
            x: event.x,
            y: event.y,
            buttons: event.buttons,
            source: event.source,
            raw_offset,
            raw_len,
            raw_sha256: sha256_hex(raw),
            raw_order_inversion,
            raw_prev_ts_ns: previous_raw_ts,
            raw_prev_offset: previous_raw_offset,
        });
        previous_raw_ts = Some(event.ts_ns);
        previous_raw_offset = Some(raw_offset);
    }

    events.sort_by_key(|event| (event.ts_ns, event.raw_offset));
    let parsed_count = events.len();
    let mut deduped = Vec::with_capacity(parsed_count);
    let mut duplicate_groups = Vec::with_capacity(parsed_count);
    let mut records_dropped_duplicate = 0usize;

    let mut start = 0usize;
    while start < events.len() {
        let mut end = start + 1;
        while end < events.len() && events[end].ts_ns == events[start].ts_ns {
            end += 1;
        }
        let selected = end - 1;
        let mut records = Vec::with_capacity(end - start);
        for (index, event) in events.iter().enumerate().take(end).skip(start) {
            records.push(DuplicateRecord {
                seq: event.seq,
                raw_offset: event.raw_offset,
                raw_len: event.raw_len,
                selected: index == selected,
            });
        }
        records_dropped_duplicate += end - start - 1;
        deduped.push(events[selected].clone());
        duplicate_groups.push(records);
        start = end;
    }

    let min_ts_ns = deduped.first().map(|event| event.ts_ns);
    let max_ts_ns = deduped.last().map(|event| event.ts_ns);
    Ok(ParsedLog {
        records_parsed: parsed_count + bad_json_records,
        records_kept: deduped.len(),
        records_dropped_duplicate,
        records_dropped_bad_json: bad_json_records,
        events: deduped,
        duplicates: duplicate_groups,
        min_ts_ns,
        max_ts_ns,
    })
}

pub fn raw_slice(path: &Path, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0u8; len as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn preserves_offsets_and_dedupes_last() {
        let input = br#"{"seq":1,"ts_ns":10,"x":1.0,"y":2.0}
{"seq":2,"ts_ns":20,"x":3.0,"y":4.0}
{"seq":3,"ts_ns":10,"x":5.0,"y":6.0}
"#;
        let log = parse(input, 100).unwrap();
        assert_eq!(log.records_parsed, 3);
        assert_eq!(log.records_kept, 2);
        assert_eq!(log.records_dropped_duplicate, 1);
        assert_eq!(log.events[0].seq, 3);
        assert!(log.events[0].raw_order_inversion);
        assert_eq!(log.events[0].raw_offset, 74);
    }

    #[test]
    fn empty_log_is_valid() {
        let log = parse(b"", 100).unwrap();
        assert_eq!(log.records_parsed, 0);
        assert_eq!(log.records_kept, 0);
        assert!(log.events.is_empty());
    }
}
