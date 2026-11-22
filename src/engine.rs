use crate::evidence;
use crate::gst_backend;
use crate::io::{absolute_path, build_anchors, copy_readonly, file_sha256, probe_wav};
use crate::model::{
    ANCHOR_COUNT, CalibrationConfig, FinalArtifact, GainDecision, LoudnessStats, RunState,
    SourceInfo, round3,
};
use crate::resource;
use anyhow::{Context, Result, bail, ensure};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone)]
pub struct AnalysisOutcome {
    pub stats: LoudnessStats,
    pub pipeline: String,
}

#[derive(Debug, Clone)]
pub struct RunPaths {
    pub root: PathBuf,
    pub state: PathBuf,
    pub checkpoints: PathBuf,
    pub segments: PathBuf,
    pub output: PathBuf,
    pub evidence: PathBuf,
    pub raw: PathBuf,
    pub source_copy: PathBuf,
}

impl RunPaths {
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            state: root.join("state.json"),
            checkpoints: root.join("checkpoints"),
            segments: root.join("segments"),
            output: root.join("output"),
            evidence: root.join("evidence"),
            raw: root.join("evidence").join("raw"),
            source_copy: root.join("source").join("original.wav"),
        }
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.root,
            &self.checkpoints,
            &self.segments,
            &self.output,
            &self.evidence,
            &self.raw,
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ProgressUpdate {
    pub run_id: String,
    pub status: String,
    pub next_segment: usize,
    pub anchor: Option<String>,
    pub message: String,
    pub mode: String,
}

type ProgressCallback = Arc<dyn Fn(ProgressUpdate) + Send + Sync>;

#[derive(Clone, Default)]
pub struct ProcessOptions {
    pub pause_after: Option<usize>,
    pub pause_flag: Option<Arc<AtomicBool>>,
    pub progress: Option<ProgressCallback>,
}

#[derive(Debug, Clone)]
pub struct ProcessOutcome {
    pub state: RunState,
    pub paused: bool,
    pub complete: bool,
}

impl ProcessOptions {
    fn emit(&self, state: &RunState, message: impl Into<String>) {
        if let Some(callback) = &self.progress {
            callback(ProgressUpdate {
                run_id: state.run_id.clone(),
                status: state.status.clone(),
                next_segment: state.next_segment,
                anchor: state.completed.last().map(|item| item.anchor_id.clone()),
                message: message.into(),
                mode: state.resource.mode.clone(),
            });
        }
    }

    fn pause_requested(&self, completed_index: usize) -> bool {
        if self.pause_after == Some(completed_index) {
            return true;
        }
        self.pause_flag
            .as_ref()
            .map(|flag| flag.swap(false, Ordering::SeqCst))
            .unwrap_or(false)
    }
}

pub fn init_run(
    source_path: &Path,
    run_dir: &Path,
    config: CalibrationConfig,
) -> Result<(RunPaths, RunState)> {
    resource::ensure_gstreamer()?;
    let source_path = absolute_path(source_path)?;
    ensure!(
        source_path.exists(),
        "原始材料不存在: {}",
        source_path.display()
    );
    let wav = probe_wav(&source_path)?;
    let anchors = build_anchors(&wav)?;
    ensure!(anchors.len() == ANCHOR_COUNT, "锚点数量不是三段");

    let paths = RunPaths::new(run_dir);
    paths.ensure()?;
    let source_hash = file_sha256(&source_path)?;
    let copy_hash = copy_readonly(&source_path, &paths.source_copy)?;
    ensure!(
        source_hash == copy_hash,
        "原始材料与只读副本哈希不一致，拒绝开始"
    );
    let readonly = fs::metadata(&source_path)?.permissions().readonly()
        || (std::os::unix::fs::PermissionsExt::mode(&fs::metadata(&source_path)?.permissions())
            & 0o222
            == 0);
    let source = SourceInfo {
        path: source_path.to_string_lossy().into_owned(),
        sha256: source_hash,
        file_size: wav.file_size,
        data_offset: wav.data_offset,
        data_size: wav.data_size,
        sample_rate: wav.sample_rate,
        channels: wav.channels,
        bits_per_sample: wav.bits_per_sample,
        sample_format: wav.sample_format,
        frame_count: wav.frame_count,
        duration_ns: wav.duration_ns,
        original_copy_path: paths.source_copy.to_string_lossy().into_owned(),
        original_copy_sha256: copy_hash,
        original_mode_readonly: readonly,
    };
    let snapshot = resource::probe(&source, &config, &paths.root)?;
    if let Err(error) = resource::preflight_resource(&snapshot) {
        let _ = crate::io::atomic_json(
            &paths.evidence.join("resource-refusal.json"),
            &serde_json::json!({
                "at": crate::model::now_rfc3339(),
                "status": "refused_before_processing",
                "error": format!("{error:#}"),
                "resource": snapshot,
                "source": source
            }),
        );
        return Err(error);
    }
    let degradation = resource::degradation_for(&snapshot, source.sample_rate);

    let now = crate::model::now_rfc3339();
    let run_id = format!(
        "{}-{}",
        Path::new(&source.path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("audio"),
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let mut state = RunState {
        schema_version: crate::model::SCHEMA_VERSION,
        run_id,
        created_at: now.clone(),
        updated_at: now,
        status: "running".to_string(),
        source,
        config,
        anchors,
        resource: snapshot,
        degradation,
        next_segment: 0,
        completed: Vec::new(),
        resume_count: 0,
        last_pause_reason: None,
        final_output: None,
        events: Vec::new(),
    };
    state.push_event(
        "run_initialized",
        None,
        format!(
            "source_sha256={} data_offset={} frames={}",
            state.source.sha256, state.source.data_offset, state.source.frame_count
        ),
    );
    state.push_event(
        "resource_selected",
        None,
        format!(
            "mode={} memory={}MB/{}MB disk={}MB/{}MB reasons={}",
            state.resource.mode,
            state.resource.memory_available_mb,
            state.resource.memory_required_mb,
            state.resource.disk_available_mb,
            state.resource.disk_required_mb,
            if state.resource.reasons.is_empty() {
                "none".to_string()
            } else {
                state.resource.reasons.join(";")
            }
        ),
    );
    evidence::write_initial_evidence(&paths, &state)?;
    evidence::save_state(&paths, &state)?;
    Ok((paths, state))
}

pub fn load_state(paths: &RunPaths) -> Result<RunState> {
    evidence::load_state(paths)
}

pub fn resume(paths: &RunPaths, options: ProcessOptions) -> Result<ProcessOutcome> {
    let mut state = load_state(paths)?;
    resume_preflight(paths, &state)?;
    if state.status == "completed" {
        evidence::finalize_run(paths, &state)?;
        return Ok(ProcessOutcome {
            state,
            paused: false,
            complete: true,
        });
    }
    state.resume_count += 1;
    state.push_event(
        "resume_preflight_passed",
        None,
        format!(
            "resume_count={} from_next_segment={} completed={}",
            state.resume_count,
            state.next_segment,
            state.completed.len()
        ),
    );
    evidence::save_state(paths, &state)?;
    process(paths, state, options)
}

pub fn resume_preflight(paths: &RunPaths, state: &RunState) -> Result<()> {
    let preflight = evidence::preflight(paths, state)?;
    if preflight.action != "continue" {
        bail!(
            "恢复前校验失败，断点与证据已保留: {}",
            preflight.reasons.join("; ")
        );
    }
    Ok(())
}

pub fn process(
    paths: &RunPaths,
    state: RunState,
    options: ProcessOptions,
) -> Result<ProcessOutcome> {
    let result = process_inner(paths, state, options);
    if let Err(error) = &result
        && let Ok(mut failed) = evidence::load_state(paths)
    {
        failed.status = "failed".to_string();
        failed.push_event("run_failed", None, format!("{error:#}"));
        let _ = evidence::save_state(paths, &failed);
        let _ = crate::io::atomic_json(
            &paths.evidence.join("failure.json"),
            &serde_json::json!({
                "at": crate::model::now_rfc3339(),
                "status": "failed",
                "next_segment": failed.next_segment,
                "completed": failed.completed.len(),
                "error": format!("{error:#}"),
                "last_event": failed.events.last()
            }),
        );
    }
    result
}

fn process_inner(
    paths: &RunPaths,
    mut state: RunState,
    options: ProcessOptions,
) -> Result<ProcessOutcome> {
    resource::ensure_gstreamer()?;
    if state.status == "completed" {
        return Ok(ProcessOutcome {
            state,
            paused: false,
            complete: true,
        });
    }
    resource::preflight_resource(&state.resource)?;
    state.status = "running".to_string();
    state.last_pause_reason = None;
    evidence::save_state(paths, &state)?;
    options.emit(&state, "开始处理下一锚点");

    while state.next_segment < state.anchors.len() {
        let index = state.next_segment;
        let anchor = state.anchors[index].clone();
        state.push_event(
            "segment_started",
            Some(&anchor.id),
            format!(
                "t={:.9}s frame={} file_bytes={}..{} mode={}",
                anchor.anchor_ns as f64 / 1e9,
                anchor.anchor_frame,
                anchor.file_start_byte,
                anchor.file_end_byte,
                state.resource.mode
            ),
        );
        evidence::save_state(paths, &state)?;

        let output_rate = state
            .degradation
            .as_ref()
            .map(|record| record.output_sample_rate)
            .unwrap_or(state.source.sample_rate);
        let analysis_rate = if state.degradation.is_some() {
            output_rate
        } else {
            state.source.sample_rate
        };
        let analysis = gst_backend::analyze_window(
            Path::new(&state.source.path),
            anchor.segment_start_ns,
            anchor.segment_end_ns,
            analysis_rate,
            state.source.channels,
        )
        .with_context(|| format!("锚点 {} 原始响度分析失败", anchor.id))?;
        let gain = choose_gain(&analysis.stats, &state.config)?;

        let segment_output =
            paths
                .segments
                .join(format!("segment-{}-{}.wav", index + 1, anchor.id));
        let render_pipeline = gst_backend::render_window(
            Path::new(&state.source.path),
            &segment_output,
            anchor.segment_start_ns,
            anchor.segment_end_ns,
            gain.gain_linear,
            output_rate,
            state.source.channels,
        )
        .with_context(|| format!("锚点 {} GStreamer 校准渲染失败", anchor.id))?;
        let output_wav = probe_wav(&segment_output)?;
        let expected_frames = ((anchor.segment_end_ns - anchor.segment_start_ns) as u128
            * output_rate as u128
            / 1_000_000_000u128) as u64;
        ensure!(
            output_wav.frame_count.abs_diff(expected_frames) <= 1,
            "锚点 {} 输出帧数 {} 与预期 {} 不一致",
            anchor.id,
            output_wav.frame_count,
            expected_frames
        );
        let after_analysis = gst_backend::analyze_window(
            &segment_output,
            0,
            output_wav.duration_ns,
            output_rate,
            state.source.channels,
        )
        .with_context(|| format!("锚点 {} 输出复测失败", anchor.id))?;
        let output_sha256 = file_sha256(&segment_output)?;
        let probe_bytes = state
            .degradation
            .as_ref()
            .map(|record| record.anchor_probe_bytes)
            .unwrap_or(4096);
        let raw_name = format!("{}-{}.bin", anchor.id, anchor.file_end_byte);
        let raw_path = paths.raw.join(raw_name);
        let (probe_start, probe_len, probe_sha) = crate::io::store_anchor_raw(
            Path::new(&state.source.original_copy_path),
            &raw_path,
            anchor.file_end_byte,
            probe_bytes,
        )?;

        let checkpoint_path = paths.checkpoints.join(format!("anchor-{}.json", anchor.id));
        let mut checkpoint = crate::model::AnchorCheckpoint {
            anchor_id: anchor.id.clone(),
            segment_index: anchor.segment_index,
            status: "complete".to_string(),
            anchor_ns: anchor.anchor_ns,
            anchor_seconds: anchor.anchor_ns as f64 / 1e9,
            anchor_frame: anchor.anchor_frame,
            original_anchor_file_byte: anchor.file_end_byte,
            segment_start_ns: anchor.segment_start_ns,
            segment_end_ns: anchor.segment_end_ns,
            segment_start_frame: anchor.segment_start_frame,
            segment_end_frame: anchor.segment_end_frame,
            payload_start_byte: anchor.payload_start_byte,
            payload_end_byte: anchor.payload_end_byte,
            file_start_byte: anchor.file_start_byte,
            file_end_byte: anchor.file_end_byte,
            analysis_pipeline: analysis.pipeline,
            render_pipeline,
            before: analysis.stats.clone(),
            gain,
            after: after_analysis.stats.clone(),
            delta_db: round3(after_analysis.stats.rms_dbfs - analysis.stats.rms_dbfs),
            target_error_db: round3(after_analysis.stats.rms_dbfs - state.config.target_rms_dbfs),
            output_wav: segment_output.to_string_lossy().into_owned(),
            output_sha256,
            output_bytes: output_wav.file_size,
            output_sample_rate: output_rate,
            raw_probe_file: raw_path.to_string_lossy().into_owned(),
            raw_probe_start_byte: probe_start,
            raw_probe_end_byte: probe_start + probe_len,
            raw_probe_sha256: probe_sha,
            raw_probe_bytes: probe_len,
            checkpoint_path: checkpoint_path.to_string_lossy().into_owned(),
            checkpoint_sha256: String::new(),
            completed_at: crate::model::now_rfc3339(),
            pause_after: false,
        };
        evidence::save_checkpoint(paths, &mut checkpoint)?;
        state.completed.push(checkpoint);
        state.next_segment += 1;
        state.push_event(
            "anchor_checkpoint_saved",
            Some(&anchor.id),
            format!(
                "next_segment={} before={:.3}dB after={:.3}dB gain={:+.3}dB output_sha256={}",
                state.next_segment,
                state.completed[index].before.rms_dbfs,
                state.completed[index].after.rms_dbfs,
                state.completed[index].gain.applied_gain_db,
                state.completed[index].output_sha256
            ),
        );
        evidence::save_state(paths, &state)?;
        options.emit(
            &state,
            format!(
                "锚点 {} 已保存：{:.3} → {:.3} dBFS",
                anchor.id,
                state.completed[index].before.rms_dbfs,
                state.completed[index].after.rms_dbfs
            ),
        );

        if options.pause_requested(index) {
            state.status = "paused".to_string();
            state.last_pause_reason = Some(format!("paused_after_anchor_{}", anchor.id));
            state.completed[index].pause_after = true;
            evidence::save_checkpoint(paths, &mut state.completed[index])?;
            state.push_event(
                "paused_at_anchor",
                Some(&anchor.id),
                format!(
                    "next_segment={} output_sha256={}",
                    state.next_segment, state.completed[index].output_sha256
                ),
            );
            evidence::save_state(paths, &state)?;
            options.emit(&state, format!("已在锚点 {} 暂停，断点已保存", anchor.id));
            return Ok(ProcessOutcome {
                state,
                paused: true,
                complete: false,
            });
        }
    }

    finalize_run(paths, &mut state)?;
    options.emit(&state, "三段校准完成，最终输出与证据已生成");
    Ok(ProcessOutcome {
        state,
        paused: false,
        complete: true,
    })
}

fn choose_gain(stats: &LoudnessStats, config: &CalibrationConfig) -> Result<GainDecision> {
    ensure!(stats.rms_dbfs.is_finite(), "原始响度统计无效");
    let requested = config.target_rms_dbfs - stats.rms_dbfs;
    let mut applied = requested;
    let mut cap = None;
    if applied < config.min_gain_db {
        applied = config.min_gain_db;
        cap = Some(format!("min_gain_db={}", config.min_gain_db));
    }
    if applied > config.max_gain_db {
        applied = config.max_gain_db;
        cap = Some(format!("max_gain_db={}", config.max_gain_db));
    }
    let predicted_peak = stats.peak_dbfs + applied;
    if predicted_peak > config.peak_ceiling_dbfs {
        applied = config.peak_ceiling_dbfs - stats.peak_dbfs;
        cap = Some(format!("peak_ceiling_dbfs={}", config.peak_ceiling_dbfs));
    }
    ensure!(applied.is_finite(), "计算出的增益无效");
    Ok(GainDecision {
        requested_gain_db: round3(requested),
        applied_gain_db: round3(applied),
        gain_linear: 10.0_f64.powf(applied / 20.0),
        cap,
    })
}

fn finalize_run(paths: &RunPaths, state: &mut RunState) -> Result<()> {
    let output_rate = state
        .degradation
        .as_ref()
        .map(|record| record.output_sample_rate)
        .unwrap_or(state.source.sample_rate);
    let segment_paths: Vec<PathBuf> = state
        .completed
        .iter()
        .map(|checkpoint| PathBuf::from(&checkpoint.output_wav))
        .collect();
    let final_path = paths.output.join("calibrated.wav");
    let pipeline = gst_backend::concat_segments(
        &segment_paths,
        &final_path,
        output_rate,
        state.source.channels,
    )?;
    let final_info = probe_wav(&final_path)?;
    ensure!(
        final_info.duration_ns.abs_diff(state.source.duration_ns) <= 1_000_000,
        "最终输出时长 {}ns 与原件 {}ns 偏差超过 1ms",
        final_info.duration_ns,
        state.source.duration_ns
    );
    let final_hash = file_sha256(&final_path)?;
    state.final_output = Some(FinalArtifact {
        path: final_path.to_string_lossy().into_owned(),
        sha256: final_hash.clone(),
        bytes: final_info.file_size,
        sample_rate: output_rate,
        frame_count: final_info.frame_count,
        duration_ns: final_info.duration_ns,
    });
    state.status = "completed".to_string();
    state.push_event(
        "run_completed",
        None,
        format!(
            "final_sha256={} frames={} duration_ns={} concat_pipeline={}",
            final_hash, final_info.frame_count, final_info.duration_ns, pipeline
        ),
    );
    evidence::save_state(paths, state)?;
    evidence::finalize_run(paths, state)?;
    Ok(())
}
