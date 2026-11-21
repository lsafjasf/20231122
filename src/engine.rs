use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use gstreamer as gst;
use gstreamer::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::checkpoint;
use crate::model::{
    now_unix_ms, Checkpoint, EntryResult, EntryStatus, Playlist, PlaylistEntry, ResourceBudget,
};

const MAX_CHUNK: u64 = 16 * 1024 * 1024;

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type")]
pub enum EngineEvent {
    Log { msg: String },
    EntryDone { result: EntryResult },
    CheckpointSaved { next_index: usize },
    Paused { next_index: usize },
    Finished { entries: usize },
}

pub struct RunConfig {
    pub out_dir: PathBuf,
    pub pause_after: Option<usize>,
    pub budget_bytes: u64,
    pub pause_flag: Option<Arc<AtomicBool>>,
}

pub enum RunOutcome {
    Completed(Vec<EntryResult>),
    Paused(usize),
}

fn gst_init_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| gst::init().expect("gst init"));
}

/// Process one playlist entry through a GStreamer pipeline
/// (filesrc -> fakesink, sha256 + byte/buffer counters via pad probe).
pub fn process_entry(
    entry: &PlaylistEntry,
    index: usize,
    budget: &mut ResourceBudget,
) -> EntryResult {
    gst_init_once();
    let mut notes: Vec<String> = Vec::new();
    let started = std::time::Instant::now();
    let base = |status, bytes, buffers, sha, notes: Vec<String>| EntryResult {
        id: entry.id.clone(),
        index,
        status,
        bytes_read: bytes,
        buffers,
        sha256: sha,
        elapsed_ms: started.elapsed().as_millis() as u64,
        param_notes: notes,
    };

    let path = Path::new(&entry.uri);
    if !path.exists() {
        return base(
            EntryStatus::Missing(format!("{} not found", entry.uri)),
            0,
            0,
            None,
            notes,
        );
    }
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    // Clamp extreme parameters, recording every clamp as evidence.
    let mut chunk = entry.edge.chunk_bytes;
    if chunk == 0 {
        notes.push("chunk_bytes=0 clamped to 1".into());
        chunk = 1;
    }
    if chunk > MAX_CHUNK {
        notes.push(format!("chunk_bytes={} clamped to {}", chunk, MAX_CHUNK));
        chunk = MAX_CHUNK;
    }
    if entry.edge.timeout_ms == 0 {
        notes.push("timeout_ms=0: no time budget, degraded without reading".into());
        return base(
            EntryStatus::Degraded("timeout-budget-0".into()),
            0,
            0,
            None,
            notes,
        );
    }

    // Resource insufficiency -> degraded path (partial read).
    let read_budget = entry.edge.max_entry_bytes.min(budget.remaining_bytes);
    let degraded_reason = if size > read_budget {
        Some(format!(
            "byte-budget-exceeded(size={}, budget={})",
            size, read_budget
        ))
    } else {
        None
    };
    let num_buffers = if degraded_reason.is_some() {
        if chunk > read_budget.max(1) {
            notes.push(format!(
                "degraded: chunk_bytes={} clamped to budget {}",
                chunk,
                read_budget.max(1)
            ));
            chunk = read_budget.max(1);
        }
        // Floor: never read past the budget (buffer-granular).
        let n = (read_budget / chunk).max(1);
        notes.push(format!(
            "degraded: reading at most {} buffers of {} bytes",
            n, chunk
        ));
        Some(n)
    } else {
        None
    };

    let src = gst::ElementFactory::make("filesrc")
        .property("location", &entry.uri)
        .property("blocksize", chunk as u32)
        .build()
        .expect("filesrc");
    if let Some(n) = num_buffers {
        src.set_property("num-buffers", n.min(i32::MAX as u64) as i32);
    }
    let sink = gst::ElementFactory::make("fakesink").build().expect("fakesink");
    let pipeline = gst::Pipeline::new();
    pipeline.add_many([&src, &sink]).unwrap();
    gst::Element::link_many([&src, &sink]).unwrap();

    let hasher = Arc::new(Mutex::new(Sha256::new()));
    let counters = Arc::new(Mutex::new((0u64, 0u64)));
    {
        let pad = src.static_pad("src").unwrap();
        let hasher = Arc::clone(&hasher);
        let counters = Arc::clone(&counters);
        pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
            if let Some(gst::PadProbeData::Buffer(ref buf)) = info.data {
                let map = buf.map_readable().unwrap();
                hasher.lock().unwrap().update(&map);
                let mut c = counters.lock().unwrap();
                c.0 += map.size() as u64;
                c.1 += 1;
            }
            gst::PadProbeReturn::Ok
        });
    }

    pipeline.set_state(gst::State::Playing).unwrap();
    let bus = pipeline.bus().unwrap();
    let timeout = if entry.edge.timeout_ms == u64::MAX {
        gst::ClockTime::NONE
    } else {
        gst::ClockTime::from_mseconds(entry.edge.timeout_ms).into()
    };
    let msg = bus.timed_pop_filtered(
        timeout,
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let (bytes, buffers) = *counters.lock().unwrap();
    let digest = hasher.lock().unwrap().clone().finalize();
    let sha = Some(hex(&digest));
    let status = match msg {
        None => {
            notes.push(format!("timeout after {} ms", entry.edge.timeout_ms));
            EntryStatus::Degraded("timeout".into())
        }
        Some(m) => match m.view() {
            gst::MessageView::Error(e) => EntryStatus::Error(format!(
                "{} ({})",
                e.error(),
                e.debug().unwrap_or_default()
            )),
            _ => match degraded_reason {
                Some(r) => EntryStatus::Degraded(r),
                None => EntryStatus::Ok,
            },
        },
    };
    let _ = pipeline.set_state(gst::State::Null);
    budget.remaining_bytes = budget.remaining_bytes.saturating_sub(bytes);
    base(status, bytes, buffers, sha, notes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Pausable run loop. Saves a checkpoint after every entry; a pause request
/// (config or UI flag) stops between entries and can be resumed later.
pub fn run(
    playlist: &Playlist,
    cfg: &RunConfig,
    resume: bool,
    mut on_event: impl FnMut(EngineEvent),
) -> Result<RunOutcome> {
    gst_init_once();
    let (mut results, mut budget, start) = if resume && checkpoint::exists(&cfg.out_dir) {
        let cp = checkpoint::load(&cfg.out_dir)?;
        if cp.playlist_version != playlist.version {
            anyhow::bail!(
                "checkpoint version {} != playlist version {}",
                cp.playlist_version,
                playlist.version
            );
        }
        on_event(EngineEvent::Log {
            msg: format!("resumed from checkpoint at index {}", cp.next_index),
        });
        (cp.results, cp.budget, cp.next_index)
    } else {
        (
            Vec::new(),
            ResourceBudget {
                remaining_bytes: cfg.budget_bytes,
            },
            0,
        )
    };

    let mut processed_this_run = 0usize;
    for index in start..playlist.entries.len() {
        if let Some(flag) = &cfg.pause_flag {
            if flag.load(Ordering::SeqCst) {
                save_checkpoint(playlist, &cfg.out_dir, index, &results, &budget)?;
                on_event(EngineEvent::Paused { next_index: index });
                return Ok(RunOutcome::Paused(index));
            }
        }
        let entry = &playlist.entries[index];
        let result = process_entry(entry, index, &mut budget);
        on_event(EngineEvent::EntryDone {
            result: result.clone(),
        });
        results.push(result);
        processed_this_run += 1;
        save_checkpoint(playlist, &cfg.out_dir, index + 1, &results, &budget)?;
        on_event(EngineEvent::CheckpointSaved { next_index: index + 1 });
        if let Some(n) = cfg.pause_after {
            if processed_this_run >= n && index + 1 < playlist.entries.len() {
                on_event(EngineEvent::Paused {
                    next_index: index + 1,
                });
                return Ok(RunOutcome::Paused(index + 1));
            }
        }
    }
    std::fs::write(
        cfg.out_dir.join("results.json"),
        serde_json::to_string_pretty(&results)?,
    )?;
    on_event(EngineEvent::Finished {
        entries: results.len(),
    });
    Ok(RunOutcome::Completed(results))
}

fn save_checkpoint(
    playlist: &Playlist,
    out_dir: &Path,
    next_index: usize,
    results: &[EntryResult],
    budget: &ResourceBudget,
) -> Result<()> {
    checkpoint::save(
        out_dir,
        &Checkpoint {
            playlist_version: playlist.version.clone(),
            next_index,
            results: results.to_vec(),
            budget: budget.clone(),
            saved_unix_ms: now_unix_ms(),
        },
    )?;
    Ok(())
}

pub fn cmd(args: Vec<String>) -> Result<()> {
    let mut playlist = None;
    let mut out = None;
    let mut pause_after = None;
    let mut resume = false;
    let mut budget_bytes = u64::MAX;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--playlist" => playlist = it.next().map(PathBuf::from),
            "--out" => out = it.next().map(PathBuf::from),
            "--pause-after" => pause_after = it.next().and_then(|v| v.parse().ok()),
            "--budget-bytes" => budget_bytes = it.next().and_then(|v| v.parse().ok()).unwrap_or(u64::MAX),
            "--resume" => resume = true,
            other => anyhow::bail!("unknown arg {}", other),
        }
    }
    let playlist = Playlist::load(&playlist.context("--playlist required")?)?;
    let out = out.context("--out required")?;
    let cfg = RunConfig {
        out_dir: out,
        pause_after,
        budget_bytes,
        pause_flag: None,
    };
    let outcome = run(&playlist, &cfg, resume, |ev| {
        println!("{}", serde_json::to_string(&ev).unwrap());
    })?;
    match outcome {
        RunOutcome::Completed(r) => println!("completed: {} entries", r.len()),
        RunOutcome::Paused(i) => println!("paused at index {}", i),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EdgeParams;

    fn entry(uri: &str, edge: EdgeParams) -> PlaylistEntry {
        PlaylistEntry {
            id: "t".into(),
            title: "t".into(),
            uri: uri.into(),
            edge,
        }
    }

    fn budget() -> ResourceBudget {
        ResourceBudget {
            remaining_bytes: u64::MAX,
        }
    }

    #[test]
    fn missing_segment_is_reported() {
        let e = entry("/nonexistent/seg.bin", EdgeParams::normal());
        let r = process_entry(&e, 0, &mut budget());
        assert!(matches!(r.status, EntryStatus::Missing(_)));
        assert_eq!(r.bytes_read, 0);
    }

    #[test]
    fn zero_timeout_degrades_without_reading() {
        let dir = std::env::temp_dir().join("plvcmp_test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        std::fs::write(&f, vec![7u8; 4096]).unwrap();
        let e = entry(
            f.to_str().unwrap(),
            EdgeParams {
                timeout_ms: 0,
                ..EdgeParams::normal()
            },
        );
        let r = process_entry(&e, 0, &mut budget());
        assert!(matches!(r.status, EntryStatus::Degraded(_)));
        assert_eq!(r.bytes_read, 0);
        assert!(!r.param_notes.is_empty());
    }

    #[test]
    fn byte_budget_never_exceeded_and_extreme_chunk_clamped() {
        let dir = std::env::temp_dir().join("plvcmp_test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("b.bin");
        std::fs::write(&f, vec![3u8; 100_000]).unwrap();
        let e = entry(
            f.to_str().unwrap(),
            EdgeParams {
                chunk_bytes: u64::MAX,
                max_entry_bytes: 4096,
                ..EdgeParams::normal()
            },
        );
        let r = process_entry(&e, 0, &mut budget());
        assert!(matches!(r.status, EntryStatus::Degraded(_)));
        assert!(r.bytes_read <= 4096, "read {} > budget", r.bytes_read);
        assert!(r.param_notes.iter().any(|n| n.contains("clamped")));
    }

    #[test]
    fn full_read_hashes_whole_file() {
        let dir = std::env::temp_dir().join("plvcmp_test");
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("c.bin");
        let data: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&f, &data).unwrap();
        let e = entry(f.to_str().unwrap(), EdgeParams::normal());
        let r = process_entry(&e, 0, &mut budget());
        assert_eq!(r.status, EntryStatus::Ok);
        assert_eq!(r.bytes_read, data.len() as u64);
        assert_eq!(
            r.sha256.unwrap(),
            hex(&Sha256::digest(&data)),
        );
    }
}
