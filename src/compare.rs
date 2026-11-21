use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::model::{EntryResult, Playlist};

#[derive(Serialize, Debug)]
pub struct FieldChange {
    pub field: String,
    pub before: String,
    pub after: String,
}

#[derive(Serialize, Debug)]
#[serde(tag = "kind")]
pub enum EntryDiff {
    Added { id: String },
    Removed { id: String },
    Changed { id: String, changes: Vec<FieldChange> },
    Unchanged { id: String },
}

/// One sample taken on each side of the boundary (the missing segment),
/// recording how that entry changed between the two playlist versions.
#[derive(Serialize, Debug)]
pub struct BoundarySample {
    pub side: String, // "before" | "after"
    pub index: usize,
    pub id: String,
    pub version_a: Option<EntryResult>,
    pub version_b: Option<EntryResult>,
    pub changes: Vec<FieldChange>,
}

#[derive(Serialize, Debug)]
pub struct DiffReport {
    pub version_a: String,
    pub version_b: String,
    pub boundary_index: Option<usize>,
    pub boundary_entry_id: Option<String>,
    pub entries: Vec<EntryDiff>,
    pub boundary_samples: Vec<BoundarySample>,
}

pub fn load_results(dir: &std::path::Path) -> Result<Vec<EntryResult>> {
    let text = std::fs::read_to_string(dir.join("results.json"))
        .with_context(|| format!("no results.json in {}", dir.display()))?;
    Ok(serde_json::from_str(&text)?)
}

fn by_id(results: &[EntryResult]) -> BTreeMap<String, EntryResult> {
    results.iter().map(|r| (r.id.clone(), r.clone())).collect()
}

pub fn field_changes(a: &EntryResult, b: &EntryResult) -> Vec<FieldChange> {
    let mut out = Vec::new();
    let mut push = |field: &str, x: String, y: String| {
        if x != y {
            out.push(FieldChange {
                field: field.to_string(),
                before: x,
                after: y,
            });
        }
    };
    push("status", format!("{:?}", a.status), format!("{:?}", b.status));
    push("bytes_read", a.bytes_read.to_string(), b.bytes_read.to_string());
    push("buffers", a.buffers.to_string(), b.buffers.to_string());
    push(
        "sha256",
        a.sha256.clone().unwrap_or_default(),
        b.sha256.clone().unwrap_or_default(),
    );
    out
}

pub fn diff(
    pa: &Playlist,
    ra: &[EntryResult],
    pb: &Playlist,
    rb: &[EntryResult],
) -> DiffReport {
    let ma = by_id(ra);
    let mb = by_id(rb);
    let mut entries = Vec::new();
    for id in ma.keys() {
        match mb.get(id) {
            None => entries.push(EntryDiff::Removed { id: id.clone() }),
            Some(b) => {
                let changes = field_changes(&ma[id], b);
                if changes.is_empty() {
                    entries.push(EntryDiff::Unchanged { id: id.clone() });
                } else {
                    entries.push(EntryDiff::Changed {
                        id: id.clone(),
                        changes,
                    });
                }
            }
        }
    }
    for id in mb.keys() {
        if !ma.contains_key(id) {
            entries.push(EntryDiff::Added { id: id.clone() });
        }
    }

    // Boundary = missing segment in playlist A; sample one entry on each side.
    let boundary_index = pa.missing_index();
    let boundary_entry_id = boundary_index.map(|i| pa.entries[i].id.clone());
    let mut boundary_samples = Vec::new();
    if let Some(i) = boundary_index {
        for (side, idx) in [("before", i.wrapping_sub(1)), ("after", i + 1)] {
            if side == "before" && i == 0 {
                continue;
            }
            if let Some(entry) = pa.entries.get(idx) {
                let va = ma.get(&entry.id).cloned();
                let vb = mb.get(&entry.id).cloned();
                let changes = match (&va, &vb) {
                    (Some(x), Some(y)) => field_changes(x, y),
                    _ => Vec::new(),
                };
                boundary_samples.push(BoundarySample {
                    side: side.to_string(),
                    index: idx,
                    id: entry.id.clone(),
                    version_a: va,
                    version_b: vb,
                    changes,
                });
            }
        }
    }

    DiffReport {
        version_a: pa.version.clone(),
        version_b: pb.version.clone(),
        boundary_index,
        boundary_entry_id,
        entries,
        boundary_samples,
    }
}

/// Pause/resume consistency: results assembled across a pause must equal an
/// uninterrupted reference run.
#[derive(Serialize, Debug)]
pub struct ConsistencyReport {
    pub consistent: bool,
    pub checked: usize,
    pub mismatches: Vec<FieldChange>,
}

pub fn consistency(resumed: &[EntryResult], reference: &[EntryResult]) -> ConsistencyReport {
    let mr = by_id(reference);
    let mut mismatches = Vec::new();
    let mut checked = 0;
    for r in resumed {
        checked += 1;
        if let Some(ref_) = mr.get(&r.id) {
            mismatches.extend(field_changes(r, ref_));
        } else {
            mismatches.push(FieldChange {
                field: "presence".into(),
                before: r.id.clone(),
                after: "<missing in reference>".into(),
            });
        }
    }
    ConsistencyReport {
        consistent: mismatches.is_empty() && resumed.len() == reference.len(),
        checked,
        mismatches,
    }
}

pub fn cmd(args: Vec<String>) -> Result<()> {
    let mut run_a = None;
    let mut run_b = None;
    let mut playlist_a = None;
    let mut playlist_b = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--run-a" => run_a = it.next().map(PathBuf::from),
            "--run-b" => run_b = it.next().map(PathBuf::from),
            "--playlist-a" => playlist_a = it.next().map(PathBuf::from),
            "--playlist-b" => playlist_b = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            other => anyhow::bail!("unknown arg {}", other),
        }
    }
    let ra = load_results(&run_a.context("--run-a required")?)?;
    let rb = load_results(&run_b.context("--run-b required")?)?;
    let pa = Playlist::load(&playlist_a.context("--playlist-a required")?)?;
    let pb = Playlist::load(&playlist_b.context("--playlist-b required")?)?;
    let report = diff(&pa, &ra, &pb, &rb);
    let text = serde_json::to_string_pretty(&report)?;
    match out {
        Some(p) => std::fs::write(p, text)?,
        None => println!("{}", text),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EntryStatus;

    fn result(id: &str, index: usize, sha: Option<&str>) -> EntryResult {
        EntryResult {
            id: id.into(),
            index,
            status: EntryStatus::Ok,
            bytes_read: 10,
            buffers: 1,
            sha256: sha.map(|s| s.to_string()),
            elapsed_ms: 0,
            param_notes: Vec::new(),
        }
    }

    #[test]
    fn consistency_detects_hash_change() {
        let a = vec![result("x", 0, Some("aa"))];
        let b = vec![result("x", 0, Some("bb"))];
        let report = consistency(&a, &b);
        assert!(!report.consistent);
        assert_eq!(report.mismatches.len(), 1);
        assert_eq!(report.mismatches[0].field, "sha256");
    }

    #[test]
    fn consistency_ok_for_identical_runs() {
        let a = vec![result("x", 0, Some("aa")), result("y", 1, None)];
        let report = consistency(&a, &a.clone());
        assert!(report.consistent);
        assert_eq!(report.checked, 2);
    }
}
