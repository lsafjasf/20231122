use anyhow::{anyhow, bail, Context};
use serde_json::json;
use std::path::{Path, PathBuf};

use crate::audio::{self, SAMPLE_RATE};
use crate::evidence::Evidence;
use crate::gstutil;
use crate::resources::{self, ResourceVerdict, SimulatedLimits};
use crate::state::{AnchorStatus, LoopPlan, RunState, ANCHORS};

pub struct EngineConfig {
    pub input: PathBuf,
    pub pristine_sha256: String,
    pub run_root: PathBuf,
    pub run_id: String,
    pub pause_after: Option<usize>,
    pub simulate: SimulatedLimits,
    /// Estimated disk need for the full artifact set.
    pub needed_disk_mb: u64,
}

pub enum RunOutcome {
    Completed { run_dir: PathBuf },
    Paused { run_dir: PathBuf, after_anchor: usize },
}

pub fn run(cfg: &EngineConfig) -> anyhow::Result<RunOutcome> {
    let run_dir = cfg.run_root.join(&cfg.run_id);
    std::fs::create_dir_all(run_dir.join("checkpoints"))?;
    std::fs::create_dir_all(run_dir.join("output"))?;
    let ev = Evidence::open(&run_dir)?;

    let resumed = run_dir.join(crate::state::STATE_FILE).exists();
    let mut state = if resumed {
        let s = RunState::load(&run_dir)?;
        if s.input.sha256 != audio::sha256_file(&cfg.input)? {
            bail!("input file changed since pause; refusing to resume");
        }
        ev.log("resume", &json!({ "run_id": cfg.run_id, "next_anchor": ANCHORS[s.next_anchor.min(2)] }));
        s
    } else {
        let input_hash = audio::sha256_file(&cfg.input)?;
        if input_hash != cfg.pristine_sha256 {
            bail!("original material modified: {input_hash} != pristine {}", cfg.pristine_sha256);
        }
        let s = RunState::new(
            cfg.run_id.clone(),
            crate::state::InputInfo {
                path: cfg.input.clone(),
                sha256: input_hash.clone(),
                pristine_sha256: cfg.pristine_sha256.clone(),
            },
        );
        ev.log("run_start", &json!({
            "run_id": cfg.run_id,
            "input": cfg.input,
            "input_sha256": input_hash,
            "original_preserved_readonly": true,
        }));
        s
    };

    // --- resource gate -------------------------------------------------
    let probe = resources::probe(&run_dir, &cfg.simulate);
    let (verdict, reasons) = resources::evaluate(&probe, cfg.needed_disk_mb);
    ev.log("resource_probe", &json!({
        "mem_available_mb": probe.mem_available_mb,
        "disk_available_mb": probe.disk_available_mb,
        "verdict": format!("{verdict:?}"),
    }));
    if verdict == ResourceVerdict::Insufficient {
        ev.log("resource_insufficient", &json!({ "reasons": reasons }));
        bail!("insufficient resources: {}", reasons.join("; "));
    }
    let deg = resources::degradation(&probe, verdict, reasons);
    if deg.mode == "degraded" {
        ev.log("degradation_applied", &deg);
    }
    if state.degradation.is_none() {
        state.degradation = Some(deg.clone());
        state.save(&run_dir)?;
    }
    let deg = state.degradation.clone().unwrap();

    // --- staged anchors -------------------------------------------------
    while state.next_anchor < ANCHORS.len() {
        let idx = state.next_anchor;
        match idx {
            0 => anchor_decode(&cfg, &run_dir, &ev, &deg, &mut state)?,
            1 => anchor_analyze(&run_dir, &ev, &deg, &mut state)?,
            2 => anchor_render(&run_dir, &ev, &mut state)?,
            _ => unreachable!(),
        }
        state.next_anchor += 1;
        state.save(&run_dir)?;
        if cfg.pause_after == Some(idx) && state.next_anchor < ANCHORS.len() {
            ev.log("paused", &json!({ "after_anchor": ANCHORS[idx], "next_anchor": ANCHORS[idx + 1] }));
            return Ok(RunOutcome::Paused { run_dir, after_anchor: idx });
        }
    }

    ev.log("run_complete", &json!({
        "run_id": cfg.run_id,
        "output_sha256": state.anchors[2].sha256,
    }));
    Ok(RunOutcome::Completed { run_dir })
}

fn mark_done(state: &mut RunState, idx: usize, artifact: &str, sha: String, samples: u64, bytes: u64, pos_ns: u64) {
    let a = &mut state.anchors[idx];
    a.status = AnchorStatus::Done;
    a.artifact = Some(artifact.to_string());
    a.sha256 = Some(sha);
    a.samples = Some(samples);
    a.byte_offset = Some(bytes);
    a.completed_at = Some(crate::evidence::now_utc());
    a.stream_position_ns = Some(pos_ns);
}

fn anchor_decode(
    cfg: &EngineConfig,
    run_dir: &Path,
    ev: &Evidence,
    deg: &crate::state::Degradation,
    state: &mut RunState,
) -> anyhow::Result<()> {
    ev.log("anchor_start", &json!({ "anchor": ANCHORS[0], "mode": deg.mode }));
    let pcm_path = run_dir.join("checkpoints").join("a1_pcm.f32le");
    let stats = if deg.mode == "degraded" {
        // Chunked, bounded-memory path: stream straight to the checkpoint file.
        let mut f = std::fs::File::create(&pcm_path)?;
        gstutil::decode_f32(&cfg.input, |chunk, _pts| {
            use std::io::Write;
            f.write_all(&audio::f32_to_bytes(chunk))?;
            Ok(())
        })?
    } else {
        let (pcm, stats) = gstutil::decode_to_vec(&cfg.input)?;
        std::fs::write(&pcm_path, audio::f32_to_bytes(&pcm))?;
        stats
    };
    let sha = audio::sha256_file(&pcm_path)?;
    let seconds = stats.total_samples as f64 / SAMPLE_RATE as f64;
    ev.log("anchor_done", &json!({
        "anchor": ANCHORS[0],
        "artifact": "checkpoints/a1_pcm.f32le",
        "sha256": sha,
        "samples": stats.total_samples,
        "seconds": format!("{seconds:.6}"),
        "duration_ns": stats.duration_ns,
        "chunks": stats.chunks,
    }));
    mark_done(state, 0, "checkpoints/a1_pcm.f32le", sha, stats.total_samples,
              stats.total_samples * 4, stats.duration_ns);
    Ok(())
}

fn load_pcm(run_dir: &Path) -> anyhow::Result<Vec<f32>> {
    let data = std::fs::read(run_dir.join("checkpoints").join("a1_pcm.f32le"))?;
    Ok(audio::bytes_to_f32(&data))
}

fn anchor_analyze(
    run_dir: &Path,
    ev: &Evidence,
    deg: &crate::state::Degradation,
    state: &mut RunState,
) -> anyhow::Result<()> {
    ev.log("anchor_start", &json!({ "anchor": ANCHORS[1], "mode": deg.mode }));
    let pcm = load_pcm(run_dir)?;
    let total = pcm.len();
    if total < SAMPLE_RATE as usize {
        bail!("material too short: {total} samples");
    }
    let start = total / 4;
    let center = total * 3 / 4;
    let radius = (total / 16).min(SAMPLE_RATE as usize / 2);
    let stride = deg.analysis_stride as usize;
    let end = audio::best_join_point(&pcm, start, center, radius, stride);
    let xf = (deg.crossfade_samples as usize).min((end - start) / 8);
    let seam_est = audio::seam_error_estimate(&pcm, start, end);
    let plan = LoopPlan {
        sample_rate: SAMPLE_RATE,
        total_samples: total as u64,
        loop_start_sample: start as u64,
        loop_end_sample: end as u64,
        crossfade_samples: xf as u64,
        seam_error_max: seam_est,
        loop_start_seconds: start as f64 / SAMPLE_RATE as f64,
        loop_end_seconds: end as f64 / SAMPLE_RATE as f64,
    };
    let plan_path = run_dir.join("checkpoints").join("a2_plan.json");
    std::fs::write(&plan_path, serde_json::to_string_pretty(&plan)?)?;
    let sha = audio::sha256_file(&plan_path)?;
    ev.log("anchor_done", &json!({
        "anchor": ANCHORS[1],
        "artifact": "checkpoints/a2_plan.json",
        "sha256": sha,
        "loop_start_sample": plan.loop_start_sample,
        "loop_start_seconds": format!("{:.6}", plan.loop_start_seconds),
        "loop_end_sample": plan.loop_end_sample,
        "loop_end_seconds": format!("{:.6}", plan.loop_end_seconds),
        "crossfade_samples": plan.crossfade_samples,
        "seam_error_estimate": plan.seam_error_max,
    }));
    let pos_ns = end as u64 * 1_000_000_000 / SAMPLE_RATE as u64;
    mark_done(state, 1, "checkpoints/a2_plan.json", sha, end as u64, 0, pos_ns);
    state.plan = Some(plan);
    Ok(())
}

fn anchor_render(run_dir: &Path, ev: &Evidence, state: &mut RunState) -> anyhow::Result<()> {
    ev.log("anchor_start", &json!({ "anchor": ANCHORS[2] }));
    let plan = state.plan.clone().context("no loop plan in state")?;
    let pcm = load_pcm(run_dir)?;
    let out = audio::render_loop(
        &pcm,
        plan.loop_start_sample as usize,
        plan.loop_end_sample as usize,
        plan.crossfade_samples as usize,
    );
    let wav_path = run_dir.join("output").join("loop.wav");
    gstutil::encode_wav(&out, &wav_path)?;
    let sha = audio::sha256_file(&wav_path)?;
    let (wrap_step, max_inner) = audio::seam_metrics(&out);
    let loop_len = plan.loop_end_sample - plan.loop_start_sample;
    let report = json!({
        "output": "output/loop.wav",
        "sha256": sha,
        "loop_start_sample": plan.loop_start_sample,
        "loop_start_seconds": format!("{:.6}", plan.loop_start_seconds),
        "loop_end_sample": plan.loop_end_sample,
        "loop_end_seconds": format!("{:.6}", plan.loop_end_seconds),
        "loop_length_samples": loop_len,
        "loop_length_seconds": format!("{:.6}", loop_len as f64 / SAMPLE_RATE as f64),
        "crossfade_samples": plan.crossfade_samples,
        "crossfade_seconds": format!("{:.6}", plan.crossfade_samples as f64 / SAMPLE_RATE as f64),
        "seam_wrap_step": wrap_step,
        "seam_max_inner_step": max_inner,
        "seam_ok": wrap_step <= max_inner * 1.5 + 1e-4,
    });
    std::fs::write(run_dir.join("output").join("a3_report.json"),
                   serde_json::to_string_pretty(&report)?)?;
    ev.log("anchor_done", &json!({
        "anchor": ANCHORS[2],
        "artifact": "output/loop.wav",
        "sha256": sha,
        "loop_length_samples": loop_len,
        "loop_length_seconds": format!("{:.6}", loop_len as f64 / SAMPLE_RATE as f64),
        "seam_wrap_step": wrap_step,
        "seam_max_inner_step": max_inner,
    }));
    mark_done(state, 2, "output/loop.wav", sha, loop_len, 0,
              plan.loop_end_sample * 1_000_000_000 / SAMPLE_RATE as u64);
    Ok(())
}

/// Compare two runs artifact-by-artifact. Returns a JSON report; `identical`
/// in the result means every anchor artifact hash matches.
pub fn compare_runs(a: &Path, b: &Path) -> anyhow::Result<serde_json::Value> {
    let sa = RunState::load(a)?;
    let sb = RunState::load(b)?;
    let mut rows = Vec::new();
    let mut equal = true;
    for i in 0..ANCHORS.len() {
        let (ha, hb) = (&sa.anchors[i].sha256, &sb.anchors[i].sha256);
        let same = ha.is_some() && ha == hb;
        equal &= same;
        rows.push(json!({
            "anchor": ANCHORS[i],
            "run_a_sha256": ha,
            "run_b_sha256": hb,
            "identical": same,
        }));
    }
    Ok(json!({
        "run_a": sa.run_id, "run_b": sb.run_id,
        "identical": equal, "anchors": rows,
    }))
}
