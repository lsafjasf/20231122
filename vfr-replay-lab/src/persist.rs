//! JSONL journal persistence and evidence export.

use crate::journal::JournalEntry;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

/// Append one entry as a single JSON line.
pub fn append_entry(path: impl AsRef<Path>, entry: &JournalEntry) -> std::io::Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{}", serde_json::to_string(entry)?)?;
    Ok(())
}

/// Read every JSON line back into journal entries.
pub fn read_journal(path: impl AsRef<Path>) -> std::io::Result<Vec<JournalEntry>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for (idx, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let entry: JournalEntry = serde_json::from_str(&line).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("journal line {idx}: {err}"),
            )
        })?;
        entries.push(entry);
    }
    Ok(entries)
}

/// Write a pretty-printed JSON evidence document.
pub fn write_json(path: impl AsRef<Path>, value: &serde_json::Value) -> std::io::Result<()> {
    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)
}
