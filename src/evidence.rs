use serde::Serialize;
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// RFC3339-ish UTC timestamp with millisecond precision.
pub fn now_utc() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let (y, mo, d, h, mi, s) = civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{millis:03}Z")
}

fn civil_from_unix(t: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = t.div_euclid(86400);
    let rem = t.rem_euclid(86400);
    let (h, mi, s) = ((rem / 3600) as u32, ((rem % 3600) / 60) as u32, (rem % 60) as u32);
    // Howard Hinnant's civil-from-days algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d, h, mi, s)
}

pub struct Evidence {
    path: PathBuf,
    file: Mutex<std::fs::File>,
}

impl Evidence {
    pub fn open(run_dir: &Path) -> anyhow::Result<Self> {
        let path = run_dir.join("evidence.jsonl");
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self { path, file: Mutex::new(file) })
    }

    pub fn log<T: Serialize>(&self, event: &str, fields: &T) {
        let mut map = json!({ "ts": now_utc(), "event": event });
        if let Value::Object(extra) = serde_json::to_value(fields).unwrap_or(Value::Null) {
            if let Value::Object(base) = &mut map {
                for (k, v) in extra {
                    base.insert(k, v);
                }
            }
        }
        let line = serde_json::to_string(&map).unwrap_or_default();
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
        println!("[evidence] {line}");
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
