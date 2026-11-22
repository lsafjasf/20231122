//! The pausable subtitle font coverage pipeline.
//!
//! The pipeline is organized around one repeated media object: the same
//! object (identified by content hash) is engaged across multiple
//! sessions. A session processes cues in chunks; between chunks the
//! pipeline may pause, persisting a `Checkpoint` so a later session can
//! resume exactly where it stopped. On resume, a verification window of
//! already-processed cues is re-analyzed and the before/after result
//! fingerprints are compared.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::clocks::{ClockCalibrator, ClockPair};
use crate::coverage::CoverageEngine;
use crate::evidence::EvidenceLog;
use crate::extract;
use crate::model::*;

/// Number of cues re-analyzed after resume to compare before/after.
pub const VERIFICATION_WINDOW: u64 = 8;

#[derive(Debug, Clone)]
pub struct ResourceBudget {
    /// Cues processed per chunk before the pause flag is checked.
    pub cues_per_chunk: usize,
    /// Wall-clock budget per chunk; exceeding it degrades the next chunk.
    pub max_chunk_millis: u64,
    /// Minimum MemAvailable (kB) required for the full code path.
    pub min_available_mem_kb: u64,
}

impl Default for ResourceBudget {
    fn default() -> Self {
        Self {
            cues_per_chunk: 16,
            max_chunk_millis: 2_000,
            min_available_mem_kb: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Completed,
    Paused,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub processed: u64,
    pub total: u64,
    pub message: String,
}

fn hash_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

fn available_mem_kb() -> u64 {
    if let Ok(v) = std::env::var("SFC_FAKE_MEM_KB") {
        return v.parse().unwrap_or(u64::MAX);
    }
    fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemAvailable:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(u64::MAX)
}

fn forced_degrade() -> bool {
    std::env::var("SFC_FORCE_DEGRADE").is_ok_and(|v| v == "1")
}

pub struct Pipeline {
    object_path: PathBuf,
    out_dir: PathBuf,
    object_sha256: String,
    object_id: String,
    cues: Vec<Cue>,
    clocks: ClockPair,
    engine: CoverageEngine,
    budget: ResourceBudget,
    pause_flag: Arc<AtomicBool>,
    auto_pause_after: Option<u64>,
    evidence: EvidenceLog,
    // Resumable state (mirrors Checkpoint).
    next_index: u64,
    sessions: Vec<SessionInfo>,
    cue_records: Vec<CueRecord>,
    coverage: Vec<CoverageEntry>,
    degradation_events: Vec<DegradationEvent>,
    clock_samples: Vec<ClockSample>,
    verification_window_hash: Option<String>,
    resume_comparison: Option<ResumeComparison>,
    session_seq: u64,
}

impl Pipeline {
    /// Start a fresh analysis of the media object.
    pub fn new(
        object_path: &Path,
        out_dir: &Path,
        budget: ResourceBudget,
        pause_flag: Arc<AtomicBool>,
        auto_pause_after: Option<u64>,
    ) -> Result<Self> {
        fs::create_dir_all(out_dir).with_context(|| format!("create {}", out_dir.display()))?;
        let object_sha256 = hash_file(object_path)?;
        let object_id = format!("obj-{}", &object_sha256[..16]);
        let cues = extract::extract_cues(object_path)?;
        let evidence = EvidenceLog::open(out_dir)?;
        let clocks = ClockPair::from_environment();
        let mut p = Self {
            object_path: object_path.to_path_buf(),
            out_dir: out_dir.to_path_buf(),
            object_sha256,
            object_id,
            cues,
            clocks,
            engine: CoverageEngine::new(),
            budget,
            pause_flag,
            auto_pause_after,
            evidence,
            next_index: 0,
            sessions: Vec::new(),
            cue_records: Vec::new(),
            coverage: Vec::new(),
            degradation_events: Vec::new(),
            clock_samples: Vec::new(),
            verification_window_hash: None,
            resume_comparison: None,
            session_seq: 0,
        };
        let s = p.clocks.sample();
        p.clock_samples.push(s);
        let streams = extract::describe_object(object_path).unwrap_or_default();
        p.evidence.record(
            "object_inspected",
            s.capture_ts_ms,
            s.analysis_ts_ms,
            vec![
                ("object_id", Value::from(p.object_id.clone())),
                ("object_sha256", Value::from(p.object_sha256.clone())),
                ("total_cues", Value::from(p.cues.len() as u64)),
                ("streams", Value::from(streams)),
            ],
        )?;
        Ok(p)
    }

    /// Resume from the checkpoint in `out_dir`. The media object must be
    /// the same repeated object (hash check), and the verification window
    /// is re-analyzed to compare before/after results.
    pub fn resume(
        object_path: &Path,
        out_dir: &Path,
        budget: ResourceBudget,
        pause_flag: Arc<AtomicBool>,
        auto_pause_after: Option<u64>,
    ) -> Result<Self> {
        let ckpt_path = out_dir.join("checkpoint.json");
        let raw = fs::read_to_string(&ckpt_path)
            .with_context(|| format!("no checkpoint at {}", ckpt_path.display()))?;
        let ckpt: Checkpoint = serde_json::from_str(&raw).context("parse checkpoint")?;
        anyhow::ensure!(
            ckpt.compute_hash() == ckpt.checkpoint_sha256,
            "checkpoint hash mismatch: intermediate state was tampered with"
        );
        let object_sha256 = hash_file(object_path)?;
        anyhow::ensure!(
            object_sha256 == ckpt.object_sha256,
            "media object changed since pause (expected {}, got {})",
            ckpt.object_sha256,
            object_sha256
        );
        let cues = extract::extract_cues(object_path)?;
        anyhow::ensure!(
            cues.len() as u64 == ckpt.total_cues,
            "cue count changed since pause"
        );
        let evidence = EvidenceLog::open(out_dir)?;
        let mut p = Self {
            object_path: object_path.to_path_buf(),
            out_dir: out_dir.to_path_buf(),
            object_sha256,
            object_id: ckpt.object_id.clone(),
            cues,
            clocks: ClockPair::from_environment(),
            engine: CoverageEngine::new(),
            budget,
            pause_flag,
            auto_pause_after,
            evidence,
            next_index: ckpt.next_cue_index,
            sessions: ckpt.sessions.clone(),
            cue_records: ckpt.cue_records.clone(),
            coverage: ckpt.coverage.clone(),
            degradation_events: ckpt.degradation_events.clone(),
            clock_samples: ckpt.clock_samples.clone(),
            verification_window_hash: ckpt.verification_window_hash.clone(),
            resume_comparison: None,
            session_seq: ckpt.sessions.len() as u64,
        };
        let s = p.clocks.sample();
        p.clock_samples.push(s);
        p.evidence.record(
            "resumed",
            s.capture_ts_ms,
            s.analysis_ts_ms,
            vec![
                ("object_id", Value::from(p.object_id.clone())),
                ("next_cue_index", Value::from(ckpt.next_cue_index)),
                ("checkpoint_sha256", Value::from(ckpt.checkpoint_sha256.clone())),
            ],
        )?;
        p.verify_resume_window()?;
        Ok(p)
    }

    /// Re-analyze the verification window (the cues processed just before
    /// the pause) and compare result fingerprints before vs. after resume.
    fn verify_resume_window(&mut self) -> Result<()> {
        let Some(pre_hash) = self.verification_window_hash.clone() else {
            return Ok(()); // first run never paused: nothing to compare
        };
        let processed = self.next_index;
        let window = VERIFICATION_WINDOW.min(processed);
        let start = processed - window;
        let mut post_parts = String::new();
        for i in start..processed {
            let cue = &self.cues[i as usize];
            let record = &self.cue_records[i as usize];
            let fp = self.fingerprint_cue(cue, record.mode);
            post_parts.push_str(&fp);
            post_parts.push('\n');
        }
        let post_hash = sha256_hex(post_parts.as_bytes());
        let identical = post_hash == pre_hash;
        let s = self.clocks.sample();
        self.clock_samples.push(s);
        self.evidence.record(
            "resume_verification",
            s.capture_ts_ms,
            s.analysis_ts_ms,
            vec![
                ("window_size", Value::from(window)),
                ("pre_pause_hash", Value::from(pre_hash.clone())),
                ("post_resume_hash", Value::from(post_hash.clone())),
                ("identical", Value::from(identical)),
            ],
        )?;
        self.resume_comparison = Some(ResumeComparison {
            window_size: window,
            pre_pause_hash: pre_hash,
            post_resume_hash: post_hash,
            identical,
        });
        Ok(())
    }

    /// Fingerprint of one cue's per-glyph outcomes under `mode`.
    fn fingerprint_cue(&self, cue: &Cue, mode: ProcessingMode) -> String {
        let outcomes = match mode {
            ProcessingMode::Full => self.engine.analyze_full(&cue.text),
            ProcessingMode::Degraded => self.engine.analyze_degraded(&cue.text),
        };
        let mut buf = format!("{}|{}|{:?}", cue.content_hash(), cue.text.len(), mode);
        for (ch, o) in outcomes {
            buf.push_str(&format!("|{}:{:?}", ch as u32, o.status));
        }
        sha256_hex(buf.as_bytes())
    }

    /// Decide the processing mode for the next chunk, logging degradations.
    fn choose_mode(&mut self) -> Result<ProcessingMode> {
        let mem = available_mem_kb();
        let reason = if forced_degrade() {
            Some("forced by SFC_FORCE_DEGRADE=1".to_string())
        } else if mem < self.budget.min_available_mem_kb {
            Some(format!(
                "MemAvailable {mem} kB below budget {} kB",
                self.budget.min_available_mem_kb
            ))
        } else {
            None
        };
        match reason {
            None => Ok(ProcessingMode::Full),
            Some(reason) => {
                let s = self.clocks.sample();
                self.clock_samples.push(s);
                let ev = DegradationEvent {
                    seq: self.degradation_events.len() as u64,
                    at_cue_index: self.next_index,
                    reason: reason.clone(),
                    capture_ts_ms: s.capture_ts_ms,
                    analysis_ts_ms: s.analysis_ts_ms,
                };
                self.degradation_events.push(ev);
                self.evidence.record(
                    "degraded",
                    s.capture_ts_ms,
                    s.analysis_ts_ms,
                    vec![
                        ("at_cue_index", Value::from(self.next_index)),
                        ("reason", Value::from(reason)),
                    ],
                )?;
                Ok(ProcessingMode::Degraded)
            }
        }
    }

    /// Process one cue and merge its glyphs into the ordered coverage map.
    fn process_cue(&mut self, cue_idx: u64, mode: ProcessingMode) -> Result<()> {
        let cue = self.cues[cue_idx as usize].clone();
        let outcomes = match mode {
            ProcessingMode::Full => self.engine.analyze_full(&cue.text),
            ProcessingMode::Degraded => self.engine.analyze_degraded(&cue.text),
        };
        let mut missing = 0u64;
        for (ch, o) in &outcomes {
            if o.status == CoverageStatus::Missing {
                missing += 1;
            }
            match self.coverage.iter_mut().find(|e| e.char == ch.to_string()) {
                Some(entry) => {
                    entry.occurrences += 1;
                    // Upgrade evidence quality: a full check supersedes a
                    // degraded one; Missing always wins (conservative).
                    if o.status == CoverageStatus::Missing
                        || (entry.status == CoverageStatus::DegradedCheck
                            && o.status != CoverageStatus::DegradedCheck)
                    {
                        entry.status = o.status;
                        entry.font = o.font.clone();
                    }
                }
                None => self.coverage.push(CoverageEntry {
                    char: ch.to_string(),
                    codepoint: format!("U+{:04X}", *ch as u32),
                    status: o.status,
                    font: o.font.clone(),
                    occurrences: 1,
                }),
            }
        }
        let s = self.clocks.sample();
        self.clock_samples.push(s);
        let offset = self.current_offset_estimate();
        let distinct: std::collections::BTreeSet<char> =
            cue.text.chars().filter(|c| !c.is_whitespace()).collect();
        self.cue_records.push(CueRecord {
            cue_index: cue.index,
            start_ms: cue.start_ms,
            end_ms: cue.end_ms,
            mode,
            distinct_chars: distinct.len() as u64,
            missing_chars: missing,
            capture_ts_ms: s.capture_ts_ms,
            analysis_ts_ms: s.analysis_ts_ms,
            normalized_analysis_ts_ms: ClockCalibrator::normalize(s.analysis_ts_ms, offset),
            content_hash: cue.content_hash(),
            result_fingerprint: self.fingerprint_cue(&cue, mode),
        });
        Ok(())
    }

    fn current_offset_estimate(&self) -> i64 {
        ClockCalibrator::calibrate(&self.clocks.capture, &self.clocks.analysis, &self.clock_samples)
            .estimated_offset_ms
    }

    /// Hash of the verification window ending at the current position.
    fn window_hash(&self) -> Option<String> {
        let processed = self.next_index;
        if processed == 0 {
            return None;
        }
        let window = VERIFICATION_WINDOW.min(processed);
        let start = processed - window;
        let mut parts = String::new();
        for rec in &self.cue_records[start as usize..processed as usize] {
            parts.push_str(&rec.result_fingerprint);
            parts.push('\n');
        }
        Some(sha256_hex(parts.as_bytes()))
    }

    /// Persist the checkpoint atomically (tmp file + rename).
    pub fn save_checkpoint(&mut self) -> Result<()> {
        self.verification_window_hash = self.window_hash();
        let mut ckpt = Checkpoint {
            format_version: FORMAT_VERSION,
            object_id: self.object_id.clone(),
            object_sha256: self.object_sha256.clone(),
            next_cue_index: self.next_index,
            total_cues: self.cues.len() as u64,
            sessions: self.sessions.clone(),
            cue_records: self.cue_records.clone(),
            coverage: self.coverage.clone(),
            degradation_events: self.degradation_events.clone(),
            clock_samples: self.clock_samples.clone(),
            verification_window_hash: self.verification_window_hash.clone(),
            checkpoint_sha256: String::new(),
        };
        ckpt.checkpoint_sha256 = ckpt.compute_hash();
        let json = serde_json::to_string_pretty(&ckpt)?;
        let tmp = self.out_dir.join("checkpoint.json.tmp");
        let dst = self.out_dir.join("checkpoint.json");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &dst)?;
        Ok(())
    }

    /// Run the processing loop until completion or pause.
    pub fn run(
        &mut self,
        mut on_progress: impl FnMut(Progress),
    ) -> Result<RunOutcome> {
        self.session_seq += 1;
        let session_id = format!("{}-s{:02}", self.object_id, self.session_seq);
        let s = self.clocks.sample();
        self.clock_samples.push(s);
        let session_start_idx = self.next_index;
        self.evidence.record(
            "session_started",
            s.capture_ts_ms,
            s.analysis_ts_ms,
            vec![
                ("session_id", Value::from(session_id.clone())),
                ("resume_from_cue", Value::from(self.next_index)),
            ],
        )?;
        let started_capture = s.capture_ts_ms;
        let started_analysis = s.analysis_ts_ms;

        let total = self.cues.len() as u64;
        let mut outcome = RunOutcome::Completed;
        while self.next_index < total {
            let mode = self.choose_mode()?;
            let chunk_start = Instant::now();
            let chunk_end = (self.next_index + self.budget.cues_per_chunk as u64).min(total);
            let mut degraded_mid_chunk = false;
            while self.next_index < chunk_end {
                self.process_cue(self.next_index, mode)?;
                self.next_index += 1;
                // Pause is honored between cues, not just between chunks,
                // so a pause request takes effect promptly.
                let pause_now = self.pause_flag.load(Ordering::SeqCst)
                    || self.auto_pause_after.is_some_and(|n| self.next_index >= n);
                if pause_now && self.next_index < total {
                    break;
                }
            }
            let elapsed = chunk_start.elapsed().as_millis() as u64;
            if elapsed > self.budget.max_chunk_millis {
                // Time budget blown: degrade the *next* chunk.
                degraded_mid_chunk = true;
                let s = self.clocks.sample();
                self.clock_samples.push(s);
                self.degradation_events.push(DegradationEvent {
                    seq: self.degradation_events.len() as u64,
                    at_cue_index: self.next_index,
                    reason: format!(
                        "chunk took {elapsed} ms, budget {} ms",
                        self.budget.max_chunk_millis
                    ),
                    capture_ts_ms: s.capture_ts_ms,
                    analysis_ts_ms: s.analysis_ts_ms,
                });
            }
            let s = self.clocks.sample();
            self.clock_samples.push(s);
            self.evidence.record(
                "chunk_completed",
                s.capture_ts_ms,
                s.analysis_ts_ms,
                vec![
                    ("session_id", Value::from(session_id.clone())),
                    ("cues_done", Value::from(self.next_index)),
                    ("total_cues", Value::from(total)),
                    ("mode", Value::from(format!("{mode:?}"))),
                    ("chunk_millis", Value::from(elapsed)),
                ],
            )?;
            on_progress(Progress {
                processed: self.next_index,
                total,
                message: format!("chunk done ({mode:?}, {elapsed} ms)"),
            });
            // Checkpoint after every chunk: crash-safe intermediate state.
            self.save_checkpoint()?;

            let pause_requested = self.pause_flag.load(Ordering::SeqCst)
                || self.auto_pause_after.is_some_and(|n| self.next_index >= n);
            if pause_requested && self.next_index < total {
                let s = self.clocks.sample();
                self.clock_samples.push(s);
                self.evidence.record(
                    "paused",
                    s.capture_ts_ms,
                    s.analysis_ts_ms,
                    vec![
                        ("session_id", Value::from(session_id.clone())),
                        ("next_cue_index", Value::from(self.next_index)),
                        (
                            "verification_window_hash",
                            Value::from(self.verification_window_hash.clone().unwrap_or_default()),
                        ),
                    ],
                )?;
                outcome = RunOutcome::Paused;
                break;
            }
            if degraded_mid_chunk {
                // Recorded above; the next loop iteration's choose_mode
                // re-evaluates real memory pressure.
            }
        }

        self.sessions.push(SessionInfo {
            session_id: session_id.clone(),
            started_capture_ts_ms: started_capture,
            started_analysis_ts_ms: started_analysis,
            cues_from: session_start_idx,
            cues_to: self.next_index,
            ended_by: match outcome {
                RunOutcome::Completed => "completed".to_string(),
                RunOutcome::Paused => "paused".to_string(),
            },
        });
        self.save_checkpoint()?;

        if outcome == RunOutcome::Completed {
            self.finalize_report()?;
        }
        on_progress(Progress {
            processed: self.next_index,
            total,
            message: format!("session {session_id} ended: {outcome:?}"),
        });
        Ok(outcome)
    }

    /// Build the final report, seal the evidence log, and export.
    fn finalize_report(&mut self) -> Result<()> {
        let calibration = ClockCalibrator::calibrate(
            &self.clocks.capture,
            &self.clocks.analysis,
            &self.clock_samples,
        );
        let degraded_cues = self
            .cue_records
            .iter()
            .filter(|r| r.mode == ProcessingMode::Degraded)
            .count() as u64;
        let s = self.clocks.sample();
        self.clock_samples.push(s);
        self.evidence.record(
            "report_finalized",
            s.capture_ts_ms,
            s.analysis_ts_ms,
            vec![
                ("object_id", Value::from(self.object_id.clone())),
                ("processed_cues", Value::from(self.next_index)),
                ("degraded_cues", Value::from(degraded_cues)),
                ("sessions", Value::from(self.sessions.len() as u64)),
            ],
        )?;
        let evidence_sha256 = self.evidence.file_digest()?;
        let mut report = Report {
            format_version: FORMAT_VERSION,
            report_kind: "subtitle-font-coverage".to_string(),
            object_id: self.object_id.clone(),
            object_path: self.object_path.display().to_string(),
            object_sha256: self.object_sha256.clone(),
            object_size_bytes: fs::metadata(&self.object_path).map(|m| m.len()).unwrap_or(0),
            generated_by: concat!("subtitle-font-coverage/", env!("CARGO_PKG_VERSION")).to_string(),
            clock_calibration: calibration,
            total_cues: self.cues.len() as u64,
            processed_cues: self.next_index,
            degraded_cues,
            sessions: self.sessions.clone(),
            degradation_events: self.degradation_events.clone(),
            resume_comparison: self.resume_comparison.clone(),
            coverage: self.coverage.clone(),
            cue_records: self.cue_records.clone(),
            evidence_sha256,
            report_sha256: String::new(),
        };
        report.report_sha256 = report.compute_hash();
        crate::report::write_report(&self.out_dir, &report)?;
        // The checkpoint is stale once the report exists; keep it for
        // audit but mark completion by leaving report.json as the seal.
        Ok(())
    }

    pub fn resume_comparison(&self) -> Option<&ResumeComparison> {
        self.resume_comparison.as_ref()
    }
}

/// Bail if the out_dir contains a checkpoint for a *different* object.
pub fn ensure_same_object(out_dir: &Path, object_path: &Path) -> Result<()> {
    let ckpt_path = out_dir.join("checkpoint.json");
    if !ckpt_path.exists() {
        return Ok(());
    }
    let raw = fs::read_to_string(&ckpt_path)?;
    let ckpt: Checkpoint = serde_json::from_str(&raw)?;
    let object_sha256 = hash_file(object_path)?;
    if ckpt.object_sha256 != object_sha256 {
        bail!(
            "output dir belongs to a different media object ({}); use a fresh directory",
            ckpt.object_id
        );
    }
    Ok(())
}
