use crate::acceptance::{AcceptanceReport, ResumeComparison, make_check};
use crate::checkpoint::{Checkpoint, JobState, PauseReason, hash_prefix, source_identity};
use crate::evidence::{EvidenceEvent, EvidenceWriter, write_key_evidence};
use crate::fixture::{hash_file, hex_lower, load_profile, validate_source};
use crate::metadata::FrameMetadata;
use crate::model::{
    AnalysisQuality, CameraProfile, METADATA_RECORD_BYTES, RepairConfig, SessionPaths,
};
use crate::pipeline::FramePipeline;
use crate::resource::plan_resource;
use anyhow::{Context, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{FileExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestAcceptance {
    pub frame: u32,
    pub pts_ns: u64,
    pub video_offset: u64,
    pub metadata_offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobManifest {
    pub schema_version: u32,
    pub job_id: String,
    pub created_unix_ms: u128,
    pub source_dir: String,
    pub config: RepairConfig,
    pub source_video_sha256: String,
    pub source_metadata_sha256: String,
    pub video_bytes: u64,
    pub metadata_bytes: u64,
    pub acceptance: ManifestAcceptance,
}

impl JobManifest {
    pub fn acceptance(&self) -> ManifestAcceptance {
        ManifestAcceptance {
            frame: self.config.acceptance_frame,
            pts_ns: self.config.acceptance_pts_ns(),
            video_offset: self.config.acceptance_video_offset(),
            metadata_offset: self.config.acceptance_metadata_offset(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Started {
        first_frame: u32,
        total_frames: u32,
        resumed: bool,
    },
    Frame {
        frame: u32,
        pts_ns: u64,
        video_offset: u64,
        metadata_offset: u64,
        before: FrameMetadata,
        after: FrameMetadata,
        preview_rgba: Option<Vec<u8>>,
        object_present: bool,
    },
    ChunkCommitted {
        next_frame: u32,
        total_frames: u32,
        plan: crate::model::ResourcePlan,
    },
    Paused {
        frame: u32,
        pts_ns: u64,
        video_offset: u64,
        metadata_offset: u64,
        reason: PauseReason,
    },
    ResourcePaused {
        frame: u32,
        pts_ns: u64,
        video_offset: u64,
        metadata_offset: u64,
        plan: crate::model::ResourcePlan,
    },
    Completed {
        acceptance_path: PathBuf,
        output_sha256: String,
    },
}

#[derive(Clone)]
pub struct RunOptions {
    pub pause_requested: Arc<AtomicBool>,
    pub memory_budget_override: Option<u64>,
    pub pause_before_frame_override: Option<Option<u32>>,
    pub progress: Option<Arc<dyn Fn(ProgressEvent) + Send + Sync>>,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            pause_requested: Arc::new(AtomicBool::new(false)),
            memory_budget_override: None,
            pause_before_frame_override: None,
            progress: None,
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum RunOutcome {
    Completed {
        report: AcceptanceReport,
        checkpoint: Checkpoint,
    },
    Paused {
        checkpoint: Checkpoint,
    },
    ResourcePaused {
        checkpoint: Checkpoint,
        plan: crate::model::ResourcePlan,
    },
}

pub struct Session {
    pub job_id: String,
    pub paths: SessionPaths,
    pub manifest: JobManifest,
    pub checkpoint: Checkpoint,
}

impl Session {
    pub fn prepare(
        source_dir: &Path,
        workspace: PathBuf,
        config: RepairConfig,
    ) -> anyhow::Result<Self> {
        let source_dir = source_dir
            .canonicalize()
            .with_context(|| format!("open source {}", source_dir.display()))?;
        let camera_json = source_dir.join("camera.json");
        let profile = load_profile(&camera_json)?;
        validate_source(&source_dir, &profile)?;
        anyhow::ensure!(
            profile.source_video_sha256 == hash_file(&source_dir.join("video.gray16"))?,
            "fixture manifest hash mismatch"
        );
        anyhow::ensure!(
            config.profile.source_video_sha256 == profile.source_video_sha256,
            "configuration and source profile disagree"
        );
        let paths = SessionPaths::new(workspace.clone());
        anyhow::ensure!(
            !paths.manifest.exists(),
            "workspace already contains a repair job: {}",
            workspace.display()
        );
        fs::create_dir_all(&paths.original_dir)?;
        fs::create_dir_all(&paths.output_dir)?;
        fs::create_dir_all(&paths.state_dir)?;
        fs::create_dir_all(&paths.evidence_dir)?;

        copy_read_only(&source_dir.join("video.gray16"), &paths.video)?;
        copy_read_only(&source_dir.join("metadata.bin"), &paths.metadata)?;
        copy_read_only(&camera_json, &paths.original_dir.join("camera.json"))?;
        copy_file(&paths.metadata, &paths.repaired_metadata)?;

        let source_video_sha256 = hash_file(&paths.video)?;
        let source_metadata_sha256 = hash_file(&paths.metadata)?;
        anyhow::ensure!(
            source_video_sha256 == profile.source_video_sha256,
            "copied original video hash mismatch"
        );
        let video_bytes = fs::metadata(&paths.video)?.len();
        let metadata_bytes = fs::metadata(&paths.metadata)?.len();
        anyhow::ensure!(
            video_bytes == profile.frame_bytes() * u64::from(profile.frame_count),
            "video length does not match profile"
        );
        anyhow::ensure!(
            metadata_bytes == METADATA_RECORD_BYTES as u64 * u64::from(profile.frame_count),
            "metadata length does not match profile"
        );
        let job_id = workspace
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("hdr-job")
            .to_string();
        let manifest = JobManifest {
            schema_version: 1,
            job_id: job_id.clone(),
            created_unix_ms: unix_ms(),
            source_dir: source_dir.display().to_string(),
            config,
            source_video_sha256: source_video_sha256.clone(),
            source_metadata_sha256: source_metadata_sha256.clone(),
            video_bytes,
            metadata_bytes,
            acceptance: ManifestAcceptance {
                frame: 0,
                pts_ns: 0,
                video_offset: 0,
                metadata_offset: 0,
            },
        };
        let mut manifest = manifest;
        manifest.acceptance = manifest.acceptance();
        write_json_atomic(&paths.manifest, &manifest)?;

        let mut checkpoint = Checkpoint::prepared(
            &job_id,
            &manifest.config.profile,
            source_video_sha256,
            source_metadata_sha256,
        );
        checkpoint.save(&paths)?;
        let mut evidence = EvidenceWriter::create(&paths)?;
        evidence.append(&EvidenceEvent::JobPrepared {
            job_id: job_id.clone(),
            source_video_sha256: checkpoint.source_video_sha256.clone(),
            source_metadata_sha256: checkpoint.source_metadata_sha256.clone(),
            original_copy: paths.original_dir.display().to_string(),
            acceptance_frame: manifest.acceptance.frame,
            acceptance_pts_ns: manifest.acceptance.pts_ns,
        })?;
        checkpoint.evidence_seq = evidence.seq();
        checkpoint.save(&paths)?;
        evidence.sync()?;
        Ok(Self {
            job_id,
            paths,
            manifest,
            checkpoint,
        })
    }

    pub fn open(workspace: &Path) -> anyhow::Result<Self> {
        let paths = SessionPaths::new(workspace.to_path_buf());
        let data = fs::read(&paths.manifest)?;
        let manifest: JobManifest = serde_json::from_slice(&data)?;
        let checkpoint = Checkpoint::load(&paths.checkpoint)?;
        Ok(Self {
            job_id: manifest.job_id.clone(),
            paths,
            manifest,
            checkpoint,
        })
    }

    pub fn profile(&self) -> &CameraProfile {
        &self.manifest.config.profile
    }

    pub fn run(&mut self, options: &RunOptions) -> anyhow::Result<RunOutcome> {
        let profile = self.manifest.config.profile.clone();
        let (video_hash, metadata_hash) = source_identity(&self.paths)?;
        let mut checkpoint = Checkpoint::load(&self.paths.checkpoint)?;
        checkpoint.validate(&profile, &video_hash, &metadata_hash)?;
        if checkpoint.state == JobState::Complete {
            let data = fs::read(&self.paths.acceptance)
                .context("read acceptance report of completed job")?;
            let report = serde_json::from_slice(&data)?;
            return Ok(RunOutcome::Completed { report, checkpoint });
        }
        checkpoint.verify_output_prefix(&self.paths)?;
        ensure_output_geometry(&profile, &self.paths)?;

        let resumed = checkpoint.next_frame > 0
            && matches!(
                checkpoint.state,
                JobState::Paused | JobState::ResourcePaused
            );
        let resume_from = checkpoint.next_frame;
        let mut evidence = EvidenceWriter::reopen(&self.paths, checkpoint.evidence_seq)?;
        if resumed {
            checkpoint.resumed_count = checkpoint.resumed_count.saturating_add(1);
            checkpoint.state = JobState::Running;
            checkpoint.pause_reason = None;
            evidence.append(&EvidenceEvent::Resumed {
                frame: checkpoint.next_frame,
                pts_ns: checkpoint.next_pts_ns,
                video_offset: checkpoint.next_video_offset,
                metadata_offset: checkpoint.next_metadata_offset,
                checkpoint_seq: checkpoint.checkpoint_seq,
                output_prefix_verified: true,
            })?;
            checkpoint.evidence_seq = evidence.seq();
            checkpoint.save(&self.paths)?;
        }
        emit(
            options,
            ProgressEvent::Started {
                first_frame: checkpoint.next_frame,
                total_frames: profile.frame_count,
                resumed,
            },
        );

        let mut pause_target = options
            .pause_before_frame_override
            .unwrap_or(self.manifest.config.pause_before_frame);
        if resumed && pause_target == Some(checkpoint.next_frame) {
            pause_target = None;
        }
        let budget = options
            .memory_budget_override
            .unwrap_or(self.manifest.config.memory_budget_bytes);
        let initial_plan = plan_resource(&profile, self.manifest.config.chunk_frames, budget);
        if initial_plan.quality == AnalysisQuality::Blocked {
            let outcome = pause_for_resource(
                &profile,
                &self.paths,
                &mut checkpoint,
                &mut evidence,
                options,
                &initial_plan,
            )?;
            self.checkpoint = checkpoint;
            return Ok(outcome);
        }
        evidence.append(&EvidenceEvent::ResourcePlan {
            frame: checkpoint.next_frame,
            plan: initial_plan.clone(),
        })?;
        let mut pipeline = FramePipeline::new(&profile)?;
        let video = File::open(&self.paths.video)?;
        let original_metadata = File::open(&self.paths.metadata)?;
        let output_metadata = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.paths.repaired_metadata)?;
        let frame_bytes = profile.frame_bytes() as usize;
        let mut last_plan: Option<crate::model::ResourcePlan> = Some(initial_plan);

        loop {
            let next = checkpoint.next_frame;
            if next >= profile.frame_count {
                break;
            }
            let plan = plan_resource(&profile, self.manifest.config.chunk_frames, budget);
            if plan.quality == AnalysisQuality::Blocked {
                let outcome = pause_for_resource(
                    &profile,
                    &self.paths,
                    &mut checkpoint,
                    &mut evidence,
                    options,
                    &plan,
                )?;
                self.checkpoint = checkpoint;
                return Ok(outcome);
            }
            if last_plan.as_ref() != Some(&plan) {
                evidence.append(&EvidenceEvent::ResourcePlan {
                    frame: next,
                    plan: plan.clone(),
                })?;
                last_plan = Some(plan.clone());
            }
            if options.pause_requested.load(Ordering::SeqCst) && next == checkpoint.next_frame {
                let outcome = pause_now(
                    &profile,
                    &self.paths,
                    &mut checkpoint,
                    &mut evidence,
                    options,
                    PauseReason::UserRequested,
                    &plan,
                )?;
                self.checkpoint = checkpoint.clone();
                return Ok(outcome);
            }

            let mut chunk_end = next
                .saturating_add(plan.chunk_frames)
                .min(profile.frame_count);
            if let Some(target) = pause_target
                && target > next
                && target < chunk_end
            {
                chunk_end = target;
            }
            let first_frame = next;
            for frame in first_frame..chunk_end {
                process_frame(
                    &profile,
                    &self.paths,
                    options,
                    &mut evidence,
                    &mut pipeline,
                    &video,
                    &original_metadata,
                    &output_metadata,
                    frame_bytes,
                    frame,
                    plan.pixel_step,
                    self.manifest.config.acceptance_frame,
                    resume_from,
                )?;
            }
            output_metadata.sync_data()?;
            let prefix = hash_prefix(
                &self.paths.repaired_metadata,
                u64::from(chunk_end) * METADATA_RECORD_BYTES as u64,
            )?;
            checkpoint.advance(&profile, chunk_end, &plan, prefix);
            checkpoint.state = JobState::Running;
            evidence.append(&EvidenceEvent::ChunkCommitted {
                first_frame,
                next_frame: chunk_end,
                next_pts_ns: checkpoint.next_pts_ns,
                next_video_offset: checkpoint.next_video_offset,
                next_metadata_offset: checkpoint.next_metadata_offset,
                output_prefix_sha256: checkpoint.output_prefix_sha256.clone(),
            })?;
            checkpoint.evidence_seq = evidence.seq();
            evidence.sync()?;
            checkpoint.save(&self.paths)?;
            self.checkpoint = checkpoint.clone();
            emit(
                options,
                ProgressEvent::ChunkCommitted {
                    next_frame: chunk_end,
                    total_frames: profile.frame_count,
                    plan: plan.clone(),
                },
            );

            let target_hit = pause_target == Some(chunk_end);
            let user_hit = options.pause_requested.swap(false, Ordering::SeqCst);
            if target_hit || user_hit {
                let reason = if target_hit {
                    PauseReason::AcceptancePoint
                } else {
                    PauseReason::UserRequested
                };
                let outcome = pause_now(
                    &profile,
                    &self.paths,
                    &mut checkpoint,
                    &mut evidence,
                    options,
                    reason,
                    &plan,
                )?;
                self.checkpoint = checkpoint.clone();
                return Ok(outcome);
            }
        }

        checkpoint.next_frame = profile.frame_count;
        checkpoint.next_pts_ns = profile.pts_ns(profile.frame_count);
        checkpoint.next_video_offset = profile.video_offset(profile.frame_count);
        checkpoint.next_metadata_offset = profile.metadata_offset(profile.frame_count);
        checkpoint.processed_frames = profile.frame_count;
        checkpoint.output_metadata_len =
            u64::from(profile.frame_count) * METADATA_RECORD_BYTES as u64;
        checkpoint.output_prefix_sha256 = hash_prefix(
            &self.paths.repaired_metadata,
            checkpoint.output_metadata_len,
        )?;
        checkpoint.state = JobState::Complete;
        checkpoint.pause_reason = None;
        if let Some(plan) = last_plan.as_ref() {
            checkpoint.resource_plan = Some(plan.clone());
        }
        let output_sha256 = hash_file(&self.paths.repaired_metadata)?;
        let output_video_sha256 = hash_file(&self.paths.video)?;
        anyhow::ensure!(
            output_video_sha256 == self.manifest.source_video_sha256,
            "original video was modified"
        );
        let report = build_acceptance(
            &self.job_id,
            &profile,
            &self.paths,
            &self.manifest,
            &checkpoint,
            output_sha256.clone(),
            output_video_sha256.clone(),
        )?;
        report.write(&self.paths)?;
        evidence.append(&EvidenceEvent::Completed {
            output_sha256: output_sha256.clone(),
            output_video_sha256,
            processed_frames: checkpoint.processed_frames,
        })?;
        checkpoint.evidence_seq = evidence.seq();
        evidence.sync()?;
        checkpoint.save(&self.paths)?;
        self.checkpoint = checkpoint.clone();
        emit(
            options,
            ProgressEvent::Completed {
                acceptance_path: self.paths.acceptance.clone(),
                output_sha256,
            },
        );
        Ok(RunOutcome::Completed { report, checkpoint })
    }
}

#[allow(clippy::too_many_arguments)]
fn process_frame(
    profile: &CameraProfile,
    paths: &SessionPaths,
    options: &RunOptions,
    evidence: &mut EvidenceWriter,
    pipeline: &mut FramePipeline,
    video: &File,
    original_metadata: &File,
    output_metadata: &File,
    frame_bytes: usize,
    frame: u32,
    pixel_step: u32,
    acceptance_frame: u32,
    resume_from: u32,
) -> anyhow::Result<()> {
    let video_offset = profile.video_offset(frame);
    let metadata_offset = profile.metadata_offset(frame);
    let mut frame_data = vec![0u8; frame_bytes];
    video
        .read_exact_at(&mut frame_data, video_offset)
        .with_context(|| format!("read raw frame {frame} at 0x{video_offset:X}"))?;
    let mut before_bytes = [0u8; METADATA_RECORD_BYTES];
    original_metadata
        .read_exact_at(&mut before_bytes, metadata_offset)
        .with_context(|| format!("read metadata {frame} at 0x{metadata_offset:X}"))?;
    let before = FrameMetadata::decode(&before_bytes)
        .with_context(|| format!("decode original metadata for frame {frame}"))?;
    anyhow::ensure!(before.frame == frame, "metadata frame index mismatch");
    anyhow::ensure!(
        before.pts_ns == profile.pts_ns(frame),
        "metadata PTS mismatch at frame {frame}"
    );
    anyhow::ensure!(
        before.source_offset == video_offset,
        "metadata source offset mismatch at frame {frame}"
    );

    let key_frame = profile.repeated_object.frames.contains(&frame)
        || frame == acceptance_frame
        || (resume_from > 0 && frame == resume_from);
    let repeated_expected = profile.repeated_object.frames.contains(&frame);
    let key_source = if key_frame {
        Some(frame_data.clone())
    } else {
        None
    };
    let processed = pipeline.process(
        frame_data,
        before.pts_ns,
        before.source_offset,
        profile,
        pixel_step,
        key_frame,
    )?;
    let after = before.repair(
        processed.analysis.peak_nits,
        processed.analysis.avg_nits,
        processed.analysis.object_present,
        pixel_step > 1,
        &profile.repaired_mastering,
    );
    let after_bytes = after.encode();
    output_metadata.write_all_at(&after_bytes, metadata_offset)?;

    if let Some(source_frame) = key_source {
        let key = write_key_evidence(paths, frame, &source_frame, &before_bytes, &after_bytes)?;
        evidence.append(&EvidenceEvent::FrameObservation {
            frame,
            pts_ns: before.pts_ns,
            video_offset,
            metadata_offset,
            peak_nits: processed.analysis.peak_nits,
            avg_nits: processed.analysis.avg_nits,
            object_present: processed.analysis.object_present,
            sampled_pixels: processed.analysis.sampled_pixels,
            total_pixels: processed.analysis.total_pixels,
            before: before.clone(),
            after: after.clone(),
            before_record_hex: hex_lower(&before_bytes),
            after_record_hex: hex_lower(&after_bytes),
            source_frame_sha256: key.source_frame_sha256,
            key_evidence_dir: Some(key.dir.display().to_string()),
        })?;
        anyhow::ensure!(
            processed.analysis.object_present || !repeated_expected,
            "configured repeated object was not found at frame {frame}"
        );
    }
    emit(
        options,
        ProgressEvent::Frame {
            frame,
            pts_ns: before.pts_ns,
            video_offset,
            metadata_offset,
            before,
            after,
            preview_rgba: processed.preview_rgba,
            object_present: processed.analysis.object_present,
        },
    );
    Ok(())
}

fn pause_now(
    profile: &CameraProfile,
    paths: &SessionPaths,
    checkpoint: &mut Checkpoint,
    evidence: &mut EvidenceWriter,
    options: &RunOptions,
    reason: PauseReason,
    plan: &crate::model::ResourcePlan,
) -> anyhow::Result<RunOutcome> {
    checkpoint.resource_plan = Some(plan.clone());
    checkpoint.mark_paused(reason.clone());
    evidence.append(&EvidenceEvent::Paused {
        frame: checkpoint.next_frame,
        pts_ns: checkpoint.next_pts_ns,
        video_offset: checkpoint.next_video_offset,
        metadata_offset: checkpoint.next_metadata_offset,
        reason: format!("{reason:?}").to_lowercase(),
        checkpoint_seq: checkpoint.checkpoint_seq,
    })?;
    checkpoint.evidence_seq = evidence.seq();
    evidence.sync()?;
    checkpoint.save(paths)?;
    save_pause_snapshot(paths, checkpoint)?;
    if checkpoint.next_frame >= profile.frame_count {
        return Err(anyhow!("cannot pause after the final frame"));
    }
    emit(
        options,
        ProgressEvent::Paused {
            frame: checkpoint.next_frame,
            pts_ns: checkpoint.next_pts_ns,
            video_offset: checkpoint.next_video_offset,
            metadata_offset: checkpoint.next_metadata_offset,
            reason,
        },
    );
    Ok(RunOutcome::Paused {
        checkpoint: checkpoint.clone(),
    })
}

fn pause_for_resource(
    profile: &CameraProfile,
    paths: &SessionPaths,
    checkpoint: &mut Checkpoint,
    evidence: &mut EvidenceWriter,
    options: &RunOptions,
    plan: &crate::model::ResourcePlan,
) -> anyhow::Result<RunOutcome> {
    evidence.append(&EvidenceEvent::ResourcePlan {
        frame: checkpoint.next_frame,
        plan: plan.clone(),
    })?;
    let current = checkpoint.next_frame;
    let prefix = hash_prefix(
        &paths.repaired_metadata,
        u64::from(current) * METADATA_RECORD_BYTES as u64,
    )?;
    checkpoint.advance(profile, current, plan, prefix);
    checkpoint.mark_paused(PauseReason::ResourceBudget);
    evidence.append(&EvidenceEvent::Paused {
        frame: checkpoint.next_frame,
        pts_ns: checkpoint.next_pts_ns,
        video_offset: checkpoint.next_video_offset,
        metadata_offset: checkpoint.next_metadata_offset,
        reason: "resource_budget".into(),
        checkpoint_seq: checkpoint.checkpoint_seq,
    })?;
    checkpoint.evidence_seq = evidence.seq();
    evidence.sync()?;
    checkpoint.save(paths)?;
    save_pause_snapshot(paths, checkpoint)?;
    emit(
        options,
        ProgressEvent::ResourcePaused {
            frame: checkpoint.next_frame,
            pts_ns: checkpoint.next_pts_ns,
            video_offset: checkpoint.next_video_offset,
            metadata_offset: checkpoint.next_metadata_offset,
            plan: plan.clone(),
        },
    );
    Ok(RunOutcome::ResourcePaused {
        checkpoint: checkpoint.clone(),
        plan: plan.clone(),
    })
}

fn save_pause_snapshot(paths: &SessionPaths, checkpoint: &Checkpoint) -> anyhow::Result<()> {
    write_json_atomic(&paths.state_dir.join("paused.json"), checkpoint)
}

fn ensure_output_geometry(profile: &CameraProfile, paths: &SessionPaths) -> anyhow::Result<()> {
    let expected = u64::from(profile.frame_count) * METADATA_RECORD_BYTES as u64;
    anyhow::ensure!(
        fs::metadata(&paths.repaired_metadata)?.len() == expected,
        "repaired metadata file size mismatch"
    );
    Ok(())
}

fn emit(options: &RunOptions, event: ProgressEvent) {
    if let Some(progress) = options.progress.as_ref() {
        progress(event);
    }
}

fn build_acceptance(
    job_id: &str,
    profile: &CameraProfile,
    paths: &SessionPaths,
    manifest: &JobManifest,
    checkpoint: &Checkpoint,
    output_metadata_sha256: String,
    output_video_sha256: String,
) -> anyhow::Result<AcceptanceReport> {
    let acceptance_frame = manifest.acceptance.frame;
    let mut frames = BTreeSet::new();
    frames.insert(acceptance_frame);
    frames.extend(profile.repeated_object.frames.iter().copied());
    let original_metadata = File::open(&paths.metadata)?;
    let repaired_metadata = File::open(&paths.repaired_metadata)?;
    let video = File::open(&paths.video)?;
    let frame_bytes = profile.frame_bytes() as usize;
    let mut checks = Vec::new();
    for frame in frames {
        let offset = profile.metadata_offset(frame);
        let mut before_bytes = [0u8; METADATA_RECORD_BYTES];
        let mut after_bytes = [0u8; METADATA_RECORD_BYTES];
        original_metadata.read_exact_at(&mut before_bytes, offset)?;
        repaired_metadata.read_exact_at(&mut after_bytes, offset)?;
        let before = FrameMetadata::decode(&before_bytes)?;
        let after = FrameMetadata::decode(&after_bytes)?;
        anyhow::ensure!(after.is_repaired(), "frame {frame} was not repaired");
        anyhow::ensure!(
            after.frame == frame
                && after.pts_ns == profile.pts_ns(frame)
                && after.source_offset == profile.video_offset(frame),
            "repaired metadata identity mismatch at frame {frame}"
        );
        let mut source_frame = vec![0u8; frame_bytes];
        video.read_exact_at(&mut source_frame, profile.video_offset(frame))?;
        let key = write_key_evidence(paths, frame, &source_frame, &before_bytes, &after_bytes)?;
        let name = if frame == acceptance_frame {
            format!("acceptance:{:.3}s", after.pts_ns as f64 / 1_000_000_000.0)
        } else {
            format!("repeated-object:{}", key.dir.display())
        };
        checks.push(make_check(
            name,
            &before,
            &after,
            &before_bytes,
            &after_bytes,
            &key.dir,
        ));
    }
    let resume_comparison = load_resume_comparison(paths, checkpoint)?;
    Ok(AcceptanceReport {
        schema_version: 1,
        job_id: job_id.to_string(),
        generated_unix_ms: unix_ms(),
        source_video_sha256: manifest.source_video_sha256.clone(),
        source_metadata_sha256: manifest.source_metadata_sha256.clone(),
        output_metadata_sha256,
        output_video_sha256,
        checks,
        resource_plan: checkpoint.resource_plan.clone(),
        resumed_count: checkpoint.resumed_count,
        resume_comparison,
    })
}

fn load_resume_comparison(
    paths: &SessionPaths,
    checkpoint: &Checkpoint,
) -> anyhow::Result<Option<ResumeComparison>> {
    if checkpoint.resumed_count == 0 {
        return Ok(None);
    }
    let paused_path = paths.state_dir.join("paused.json");
    if !paused_path.exists() {
        return Ok(None);
    }
    let paused: Checkpoint = Checkpoint::load(&paused_path)?;
    let prefix = hash_prefix(&paths.repaired_metadata, paused.output_metadata_len)?;
    let prefix_verified = prefix == paused.output_prefix_sha256;
    Ok(Some(ResumeComparison {
        paused_frame: paused.next_frame,
        paused_pts_ns: paused.next_pts_ns,
        paused_video_offset: paused.next_video_offset,
        paused_metadata_offset: paused.next_metadata_offset,
        resumed_from_frame: paused.next_frame,
        prefix_verified,
        uninterrupted_output_sha256: None,
        resumed_output_sha256: hash_file(&paths.repaired_metadata)?,
        output_equal: prefix_verified,
    }))
}

fn copy_read_only(source: &Path, destination: &Path) -> anyhow::Result<()> {
    copy_file(source, destination)?;
    let mut permissions = fs::metadata(destination)?.permissions();
    permissions.set_mode(0o444);
    fs::set_permissions(destination, permissions)?;
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> anyhow::Result<()> {
    let data = fs::read(source)?;
    let tmp = destination.with_extension("copy.tmp");
    let mut file = File::create(&tmp)?;
    file.write_all(&data)?;
    file.sync_all()?;
    fs::rename(tmp, destination)?;
    Ok(())
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> anyhow::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let mut file = File::create(&tmp)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(tmp, path)?;
    Ok(())
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
