//! Conservative repair pass.
//!
//! Repairs always write a *new* file. The original path is never touched and
//! writing to the same path (or an existing destination) is rejected.

use std::fs;
use std::path::{Path, PathBuf};

use crate::isobmff::{inspect_bytes, ErrorKind, Repairability, Report};

/// Outcome of a repair attempt.
pub struct RepairOutcome {
    pub output: PathBuf,
    pub bytes_written: u64,
    pub dropped_boxes: Vec<String>,
    pub trimmed_tail: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum RepairError {
    #[error("output path is the same as the input file; overwriting the original is not allowed")]
    SameAsInput,
    #[error("output file already exists; refusing to overwrite it: {0}")]
    OutputExists(PathBuf),
    #[error("no repairable problems found in {0}")]
    NothingToRepair(PathBuf),
    #[error("required box is damaged ({0}); repair would destroy media metadata and was refused")]
    RequiredBoxDamaged(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Repair `input` into `output`.
pub fn repair_file(input: &Path, output: &Path) -> Result<RepairOutcome, RepairError> {
    let input = fs::canonicalize(input)?;
    let output = match fs::canonicalize(output) {
        Ok(canonical) => {
            return Err(if canonical == input {
                RepairError::SameAsInput
            } else {
                RepairError::OutputExists(canonical)
            })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => output.to_path_buf(),
        Err(e) => return Err(RepairError::Io(e)),
    };
    if let Ok(true) = output.try_exists() {
        return Err(RepairError::OutputExists(output));
    }

    let data = fs::read(&input)?;
    let report = inspect_bytes(&data);
    if report.is_healthy() {
        return Err(RepairError::NothingToRepair(input));
    }

    for node in report.error_boxes() {
        if node.repair == Repairability::Required {
            return Err(RepairError::RequiredBoxDamaged(format!(
                "{} at offset {}",
                node.box_type, node.offset
            )));
        }
    }

    let mut out: Vec<u8> = Vec::with_capacity(data.len());
    let mut dropped_boxes = Vec::new();
    let mut trimmed_tail = false;

    // Only top-level boxes are rebuilt. Nested unknown boxes are treated as
    // required and rejected above, so a reachable report is safe to rebuild.
    let top_level: Vec<_> = report
        .boxes
        .iter()
        .filter(|b| b.depth == 0)
        .cloned()
        .collect();

    for node in top_level {
        let start = node.offset as usize;
        let end = start + node.declared_size as usize;
        match node.error {
            Some(ErrorKind::UnknownType) if node.repair == Repairability::Droppable => {
                dropped_boxes.push(format!("{} @ {}", node.box_type, node.offset));
                continue;
            }
            Some(ErrorKind::Truncated) if node.repair == Repairability::TrimTail => {
                // Keep only the complete prefix and stop rebuilding here.
                trimmed_tail = true;
                break;
            }
            Some(ErrorKind::Truncated) => {
                // Should have been rejected earlier; guard anyway.
                return Err(RepairError::RequiredBoxDamaged(format!(
                    "{} at offset {}",
                    node.box_type, node.offset
                )));
            }
            _ => {
                if end <= data.len() {
                    out.extend_from_slice(&data[start..end]);
                }
            }
        }
    }

    // A truncated mdat/other box may leave trailing bytes; never copy them.
    let bytes_written = out.len() as u64;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    // Atomic-ish write: temp file in the same directory, then rename.
    let tmp = output.with_extension("part");
    fs::write(&tmp, &out)?;
    fs::rename(&tmp, &output)?;

    Ok(RepairOutcome {
        output,
        bytes_written,
        dropped_boxes,
        trimmed_tail,
    })
}

/// Human-readable repair advice derived from a report.
pub fn repair_advice(report: &Report) -> Vec<String> {
    let mut advice = Vec::new();
    for node in report.error_boxes() {
        let line = match (node.error.unwrap(), node.repair) {
            (ErrorKind::UnknownType, Repairability::Droppable) => format!(
                "offset {}: unknown box {} can be dropped with `repair --out <new file>`",
                node.offset, node.box_type
            ),
            (ErrorKind::UnknownType, _) => format!(
                "offset {}: unknown box {} lives inside a track; keep it or remux the container",
                node.offset, node.box_type
            ),
            (ErrorKind::Truncated, Repairability::TrimTail) => format!(
                "offset {}: truncated {} box; repair trims the damaged tail at this offset",
                node.offset, node.box_type
            ),
            (ErrorKind::Truncated, _) => format!(
                "offset {}: required box {} is truncated; restore from backup or remux, repair refused",
                node.offset, node.box_type
            ),
            (ErrorKind::InvalidSize, _) => format!(
                "offset {}: box {} has an invalid size; remux the container required",
                node.offset, node.box_type
            ),
            (ErrorKind::Malformed, _) => format!(
                "offset {}: box {} is malformed; remux the container required",
                node.offset, node.box_type
            ),
        };
        advice.push(line);
    }
    advice
}
