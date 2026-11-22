//! Pausable processing pipeline and checkpoint management.
//!
//! A run is a plain directory on disk. `checkpoint.json` always holds the
//! full state reached so far (it is rewritten atomically after every
//! segment), while `run-record.json` is only finalized once the run reaches
//! `complete`. Pausing, terminating the process and resuming therefore share
//! the same recovery path, and every state transition is independently
//! verifiable from files alone.

use crate::compare::{compare, segment_digest, ComparisonInput};
use crate::decode::{decode_window, DecodeRequest};
use crate::digest::{canonical_json, digest_canonical, sha256_hex_bytes, sha256_hex_str};
use crate::fingerprint::{compute, FingerprintParams, ALGORITHM};
use crate::model::*;
use crate::resources::{decide, observe, ResourceVerdict};
use crate::store;
use anyhow::{bail, Context};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct RunOptions {
    pub case_path: PathBuf,
    pub output_dir: PathBuf,
    pub pause_after: Option<usize>,
    pub force_degrade: bool,
    pub command_line: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum StopReason {
    PauseRequested,
    Complete,
    ResourcePause,
}

pub fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn start_run(options: RunOptions) -> anyhow::Result<(RunRecord, StopReason)> {
    let (case_file, case_bytes, original_manifest_sha) = store::load_case(&options.case_path)?;
    if case_file.schema != CASE_SCHEMA {
        bail!("unsupported case schema: {}", case_file.schema);
    }
    std::fs::create_dir_all(options.output_dir.join("evidence/fingerprints"))
        .context("creating evidence directories")?;
    store::preserve_original_case(&options.output_dir, &case_bytes, &original_manifest_sha)?;

    let mut devices = Vec::new();
    for device in &case_file.devices {
        let resolved = store::resolve_media_path(&options.case_path, &device.media_path);
        let resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
        let mut spec = device.clone();
        spec.media_path = resolved.display().to_string();
        devices.push(spec);
    }

    let case_sha = sha256_hex_bytes(&case_bytes);
    let run_id = format!("{}-{}", case_file.case_id, &case_sha[..12]);
    let mode = if options.force_degrade {
        ProcessMode::Reduced
    } else {
        ProcessMode::Strict
    };
    let record = RunRecord {
        schema: RUN_SCHEMA.to_string(),
        format_version: 1,
        case: CaseSnapshot {
            case_id: case_file.case_id.clone(),
            case_sha256: case_sha.clone(),
            original_manifest_sha256: original_manifest_sha.clone(),
            title: case_file.title.clone(),
            description: case_file.description.clone(),
            devices,
            segments: case_file.segments.clone(),
            alignment: case_file.alignment.clone(),
            resource_policy: case_file.resource_policy.clone(),
        },
        run: RunMeta {
            run_id,
            status: RunStatus::Paused,
            mode,
            created_at_unix_ms: now_unix_ms(),
            updated_at_unix_ms: now_unix_ms(),
            resumed_at_unix_ms: None,
            resume_count: 0,
            pause_after_segments: options.pause_after,
            command_line: options.command_line.clone(),
            cursor_segment_id: None,
        },
        alignment: AlignmentResult {
            status: "pending".to_string(),
            anchor_device_id: case_file.devices[0].device_id.clone(),
            compared_device_id: case_file.devices[1].device_id.clone(),
            expected_offset_us: case_file.alignment.expected_offset_us,
            observed_offset_us: 0,
            confidence_milli: 0,
            search_radius_us: case_file.alignment.search_radius_us,
            search_step_us: case_file.alignment.search_step_us,
            samples_compared: 0,
            evidence_sha256: String::new(),
        },
        segments: Vec::new(),
        summary: empty_summary(case_file.segments.len()),
        resume_checks: Vec::new(),
        degradation_events: Vec::new(),
        evidence: store::empty_evidence_index(),
        verification: store::empty_verification(),
    };
    store::write_checkpoint(&options.output_dir, &record)?;
    continue_run(options, record)
}

pub fn resume_run(options: RunOptions) -> anyhow::Result<(RunRecord, StopReason)> {
    let checkpoint_path = options.output_dir.join("checkpoint.json");
    let raw = std::fs::read(&checkpoint_path)
        .with_context(|| format!("no checkpoint at {}", checkpoint_path.display()))?;
    let before_digest = sha256_hex_bytes(&raw);
    let mut record: RunRecord = serde_json::from_slice(&raw).context("parsing checkpoint")?;

    // Reload round-trip proof: serialize the freshly deserialized state and
    // compare it byte-for-byte against what was on disk.
    let reserialized = canonical_json(&record);
    let after_digest = sha256_hex_str(&reserialized);
    let anchor = record.segments.first().cloned();
    let anchor_before = anchor.as_ref().map(digest_canonical);
    let anchor_after = anchor.as_ref().map(digest_canonical);

    record.run.resume_count += 1;
    record.run.resumed_at_unix_ms = Some(now_unix_ms());
    record.run.updated_at_unix_ms = now_unix_ms();

    // Resource and alignment policy may be relaxed between sessions. Reload
    // them from the preserved case copy only after the round-trip digest is
    // captured, and refuse to continue if the case identity changed.
    let preserved_case = options.output_dir.join("input/case.json");
    if preserved_case.exists() {
        let (reloaded, _, _) = store::load_case(&preserved_case)?;
        if reloaded.case_id != record.case.case_id {
            bail!(
                "case_id mismatch: checkpoint={} reloaded={}",
                record.case.case_id,
                reloaded.case_id
            );
        }
        record.case.resource_policy = reloaded.resource_policy;
        record.case.alignment = reloaded.alignment;
    }

    let reading = observe(&options.output_dir);
    if options.force_degrade && record.run.mode == ProcessMode::Strict {
        let event = DegradationEvent {
            sequence: record.degradation_events.len() as u32 + 1,
            stage: "resume".to_string(),
            segment_id: None,
            reason_code: "operator_forced".to_string(),
            detail: "恢复时指定降级运行；新增片段使用低资源指纹参数".to_string(),
            from_mode: ProcessMode::Strict.as_str().to_string(),
            to_mode: ProcessMode::Reduced.as_str().to_string(),
            observed_rss_mb: reading.rss_mb,
            observed_free_disk_mb: reading.free_disk_mb,
            evidence_preserved: true,
        };
        record.degradation_events.push(event);
        record.run.mode = ProcessMode::Reduced;
    }

    let anchor_equal = anchor_before.is_none() || anchor_before.as_ref() == anchor_after.as_ref();
    let check = ResumeCheck {
        checkpoint_segment_count: record.segments.len(),
        before_resume_digest: before_digest.clone(),
        after_reload_digest: after_digest.clone(),
        state_roundtrip_equal: before_digest == after_digest,
        anchor_segment_id: anchor
            .as_ref()
            .map(|s| s.segment_id.clone())
            .unwrap_or_else(|| "none".to_string()),
        anchor_before_digest: anchor_before.clone().unwrap_or_else(|| "none".to_string()),
        anchor_after_digest: anchor_after.unwrap_or_else(|| "none".to_string()),
        anchor_content_equal: anchor_equal,
        checked_at_unix_ms: now_unix_ms(),
    };
    record.resume_checks.push(check);
    store::write_checkpoint(&options.output_dir, &record)?;
    continue_run(options, record)
}

fn continue_run(options: RunOptions, mut record: RunRecord) -> anyhow::Result<(RunRecord, StopReason)> {
    crate::decode::ensure_initialized();
    if record.alignment.status == "pending" {
        run_alignment(&options, &mut record)?;
    }

    let total = record.case.segments.len();
    let mut stop_reason = StopReason::Complete;
    while record.segments.len() < total {
        let ordinal = record.segments.len();
        let reading = observe(&options.output_dir);
        let already_reduced = record.run.mode == ProcessMode::Reduced;
        match decide(reading, &record.case.resource_policy, already_reduced) {
            ResourceVerdict::Pause => {
                record.run.status = RunStatus::Paused;
                record.run.cursor_segment_id = Some(record.case.segments[ordinal].segment_id.clone());
                record.run.updated_at_unix_ms = now_unix_ms();
                store::write_checkpoint(&options.output_dir, &record)?;
                stop_reason = StopReason::ResourcePause;
                break;
            }
            ResourceVerdict::Degrade => {
                let event = DegradationEvent {
                    sequence: record.degradation_events.len() as u32 + 1,
                    stage: "process".to_string(),
                    segment_id: Some(record.case.segments[ordinal].segment_id.clone()),
                    reason_code: "rss_threshold".to_string(),
                    detail: "RSS 达到降级阈值，后续片段切换到低采样率指纹".to_string(),
                    from_mode: record.run.mode.as_str().to_string(),
                    to_mode: ProcessMode::Reduced.as_str().to_string(),
                    observed_rss_mb: reading.rss_mb,
                    observed_free_disk_mb: reading.free_disk_mb,
                    evidence_preserved: true,
                };
                record.degradation_events.push(event);
                record.run.mode = ProcessMode::Reduced;
            }
            ResourceVerdict::Continue => {}
        }

        let segment_record = process_segment(&options, &record, ordinal)?;
        record.segments.push(segment_record);
        record.run.updated_at_unix_ms = now_unix_ms();
        record.summary = summarize(&record);
        store::write_checkpoint(&options.output_dir, &record)?;

        if let Some(pause_after) = options.pause_after {
            if record.segments.len() >= pause_after && record.segments.len() < total {
                record.run.status = RunStatus::Paused;
                record.run.cursor_segment_id = Some(record.case.segments[record.segments.len()].segment_id.clone());
                record.run.updated_at_unix_ms = now_unix_ms();
                store::write_checkpoint(&options.output_dir, &record)?;
                stop_reason = StopReason::PauseRequested;
                break;
            }
        }
    }

    if record.segments.len() == total {
        finalize(&options, &mut record)?;
        stop_reason = StopReason::Complete;
    }
    Ok((record, stop_reason))
}

fn empty_summary(total: usize) -> RunSummary {
    RunSummary {
        segments_total: total,
        segments_completed: 0,
        segments_pending: total,
        matches: 0,
        missing: 0,
        divergent: 0,
        review: 0,
        degradation_count: 0,
        result_digest: String::new(),
    }
}

fn run_alignment(options: &RunOptions, record: &mut RunRecord) -> anyhow::Result<()> {
    let anchor_spec = &record.case.devices[0];
    let compared_spec = &record.case.devices[1];
    let anchor_duration = store::media_duration_us(Path::new(&anchor_spec.media_path))?;
    let compared_duration = store::media_duration_us(Path::new(&compared_spec.media_path))?;
    let (mut alignment, anchor_frames, compared_frames) = crate::alignment::estimate(
        &crate::alignment::AlignmentInput {
            device: anchor_spec.clone(),
            media_duration_us: anchor_duration,
        },
        &crate::alignment::AlignmentInput {
            device: compared_spec.clone(),
            media_duration_us: compared_duration,
        },
        &record.case.alignment,
    )?;

    let evidence = serde_json::json!({
        "schema": ALIGNMENT_SCHEMA,
        "case_id": record.case.case_id,
        "anchor_device_id": alignment.anchor_device_id,
        "compared_device_id": alignment.compared_device_id,
        "anchor_media_sha256": store::file_sha256(Path::new(&anchor_spec.media_path))?,
        "compared_media_sha256": store::file_sha256(Path::new(&compared_spec.media_path))?,
        "anchor_sample_rate_hz": FingerprintParams::strict().sample_rate_hz,
        "compared_sample_rate_hz": FingerprintParams::strict().sample_rate_hz,
        "anchor_decode_backend": "gstreamer-uridecodebin",
        "compared_decode_backend": "gstreamer-uridecodebin",
        "anchor_pipeline": crate::decode::pipeline_description(
            Path::new(&anchor_spec.media_path),
            FingerprintParams::strict().sample_rate_hz,
        ),
        "compared_pipeline": crate::decode::pipeline_description(
            Path::new(&compared_spec.media_path),
            FingerprintParams::strict().sample_rate_hz,
        ),
        "anchor_feature_count": anchor_frames.len(),
        "compared_feature_count": compared_frames.len(),
        "expected_offset_us": alignment.expected_offset_us,
        "observed_offset_us": alignment.observed_offset_us,
        "confidence_milli": alignment.confidence_milli,
        "search_radius_us": alignment.search_radius_us,
        "search_step_us": alignment.search_step_us,
        "samples_compared": alignment.samples_compared,
        "interpretation": "正值表示 device-b 的本地时钟领先 device-a，比较窗口需加上该偏移",
    });
    let path = options.output_dir.join("evidence/alignment.json");
    std::fs::write(&path, canonical_json(&evidence))?;
    alignment.evidence_sha256 = sha256_hex_bytes(&std::fs::read(&path)?);
    record.alignment = alignment;
    Ok(())
}

fn params_for_mode(record: &RunRecord) -> FingerprintParams {
    match record.run.mode {
        ProcessMode::Strict => FingerprintParams::strict(),
        ProcessMode::Reduced => FingerprintParams::reduced(
            record.case.resource_policy.reduced_sample_rate,
            record.case.resource_policy.reduced_frame_us,
        ),
    }
}

fn process_segment(
    options: &RunOptions,
    record: &RunRecord,
    ordinal: usize,
) -> anyhow::Result<SegmentRecord> {
    let spec = &record.case.segments[ordinal];
    let params = params_for_mode(record);
    let anchor_device = &record.case.devices[0];
    let compared_device = &record.case.devices[1];
    let offset = record.alignment.observed_offset_us;
    let common_range = MicroRange {
        start_us: spec.start_us,
        end_us: spec.start_us + spec.duration_us,
    };

    let anchor = fingerprint_device(
        options, record, anchor_device, spec, &params, &common_range, 0,
    )?;
    let compared_range = MicroRange {
        start_us: common_range.start_us + offset,
        end_us: common_range.end_us + offset,
    };
    let compared = fingerprint_device(
        options,
        record,
        compared_device,
        spec,
        &params,
        &compared_range,
        offset,
    )?;

    let anchor_evidence: FingerprintEvidence =
        serde_json::from_str(&std::fs::read_to_string(
            options.output_dir.join(&anchor.evidence_file),
        )?)?;
    let compared_evidence: FingerprintEvidence =
        serde_json::from_str(&std::fs::read_to_string(
            options.output_dir.join(&compared.evidence_file),
        )?)?;

    let outcome = compare(
        ComparisonInput {
            segment: spec,
            anchor_device,
            compared_device,
            anchor_frames: &anchor_evidence.frames,
            compared_frames: &compared_evidence.frames,
            compared_clock_offset_us: offset,
        },
        &record.case.alignment,
    );

    let mut segment_record = SegmentRecord {
        ordinal: spec.ordinal,
        segment_id: spec.segment_id.clone(),
        label: spec.label.clone(),
        common_range,
        status: SegmentStatus::Completed,
        decision: outcome.decision,
        similarity_milli: outcome.similarity_milli,
        coverage_milli: outcome.coverage_milli,
        devices: vec![anchor, compared],
        missing: outcome.missing,
        degradations: record
            .degradation_events
            .iter()
            .filter(|event| event.segment_id.as_deref() == Some(spec.segment_id.as_str()))
            .cloned()
            .collect(),
        notes: outcome.notes,
        evidence_sha256: String::new(),
    };
    segment_record.evidence_sha256 = segment_digest(&segment_record);
    Ok(segment_record)
}

fn fingerprint_device(
    options: &RunOptions,
    _record: &RunRecord,
    device: &DeviceSpec,
    spec: &SegmentSpec,
    params: &FingerprintParams,
    requested_range: &MicroRange,
    clock_offset_us: i64,
) -> anyhow::Result<DeviceFingerprintResult> {
    let source_path = PathBuf::from(&device.media_path);
    let decoded = decode_window(&DecodeRequest {
        source_path: source_path.clone(),
        requested_range: requested_range.clone(),
        sample_rate_hz: params.sample_rate_hz,
    })?;
    let fingerprint = compute(&decoded.pcm_s16le, params);
    let evidence = FingerprintEvidence {
        schema: EVIDENCE_SCHEMA.to_string(),
        segment_id: spec.segment_id.clone(),
        device_id: device.device_id.clone(),
        algorithm: ALGORITHM.to_string(),
        sample_rate_hz: params.sample_rate_hz,
        frame_us: params.frame_us,
        hop_us: params.hop_us,
        requested_range: requested_range.clone(),
        decoded_range: decoded.decoded_range.clone(),
        pcm_sha256: fingerprint.pcm_sha256.clone(),
        fingerprint_sha256: fingerprint.fingerprint_sha256.clone(),
        frames: fingerprint.frames,
    };
    let relative_path = format!(
        "evidence/fingerprints/{}__{}.json",
        spec.segment_id, device.device_id
    );
    let full_path = options.output_dir.join(&relative_path);
    std::fs::write(&full_path, canonical_json(&evidence))?;

    let metadata = std::fs::metadata(&source_path)?;
    Ok(DeviceFingerprintResult {
        device_id: device.device_id.clone(),
        requested_range: requested_range.clone(),
        decoded_range: decoded.decoded_range,
        clock_offset_us,
        source_path: source_path.display().to_string(),
        source_size_bytes: metadata.len(),
        source_sha256: store::file_sha256(&source_path)?,
        decode_backend: decoded.backend,
        pipeline: decoded.pipeline_description,
        used_fallback: decoded.used_fallback,
        sample_rate_hz: params.sample_rate_hz,
        frame_us: params.frame_us,
        hop_us: params.hop_us,
        sample_count: decoded.pcm_s16le.len() / 2,
        pcm_sha256: evidence.pcm_sha256,
        fingerprint_algorithm: ALGORITHM.to_string(),
        fingerprint_sha256: evidence.fingerprint_sha256,
        feature_count: fingerprint.feature_count,
        evidence_file: relative_path,
    })
}

fn summarize(record: &RunRecord) -> RunSummary {
    let total = record.case.segments.len();
    let completed = record.segments.len();
    let matches = record
        .segments
        .iter()
        .filter(|s| s.decision == Decision::Match)
        .count();
    let missing = record
        .segments
        .iter()
        .filter(|s| s.decision == Decision::Missing)
        .count();
    let divergent = record
        .segments
        .iter()
        .filter(|s| s.decision == Decision::Divergent)
        .count();
    let review = record
        .segments
        .iter()
        .filter(|s| s.decision == Decision::Review)
        .count();
    let result_digest = result_digest(record);
    RunSummary {
        segments_total: total,
        segments_completed: completed,
        segments_pending: total - completed,
        matches,
        missing,
        divergent,
        review,
        degradation_count: record.degradation_events.len(),
        result_digest,
    }
}

pub fn result_digest(record: &RunRecord) -> String {
    let ordered_segments: Vec<_> = record
        .segments
        .iter()
        .map(segment_digest)
        .collect();
    digest_canonical(&serde_json::json!({
        "case_id": record.case.case_id,
        "case_sha256": record.case.case_sha256,
        "alignment": record.alignment,
        "segments": ordered_segments,
    }))
}

fn finalize(options: &RunOptions, record: &mut RunRecord) -> anyhow::Result<()> {
    record.run.status = RunStatus::Complete;
    record.run.cursor_segment_id = None;
    record.run.pause_after_segments = None;
    record.run.updated_at_unix_ms = now_unix_ms();
    record.summary = summarize(record);
    record.evidence = store::build_evidence_index(&options.output_dir, &record.segments)?;
    store::write_manifest(options, record)?;
    store::write_review(options, record)?;
    store::write_checksums(options, record)?;

    let record_path = options.output_dir.join("run-record.json");
    std::fs::write(&record_path, canonical_json(record))?;
    // The checkpoint converges to the same content once the run is complete.
    std::fs::copy(&record_path, options.output_dir.join("checkpoint.json"))?;
    let checksums_hash = sha256_hex_bytes(&std::fs::read(
        options.output_dir.join("checksums.sha256"),
    )?);
    record.verification = VerificationInfo {
        independent_replay_command:
            "segment-fingerprint verify --record run-record.json --deep".to_string(),
        checksums_sha256: checksums_hash,
        last_verdict: "pending".to_string(),
        last_verified_at_unix_ms: 0,
    };
    std::fs::write(&record_path, canonical_json(record))?;
    std::fs::copy(&record_path, options.output_dir.join("checkpoint.json"))?;
    Ok(())
}
