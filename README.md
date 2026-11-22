# 字幕字体覆盖报告 — Subtitle Font Coverage (pausable pipeline)

A pausable subtitle font-coverage analysis pipeline built with **Rust +
GStreamer + GTK4**. GStreamer extracts subtitle cues from a media object;
Pango checks which installed fonts cover the glyphs used; the result is
exported as an independently reviewable report with a hash-chained
evidence log.

## Design notes (how each requirement is met)

- **Repeated object**: the pipeline is organized around one media object,
  identified by its SHA-256 (`obj-<hash16>`). The same object is engaged
  repeatedly across sessions (analyze → pause → resume); the checkpoint
  and report are keyed by the object id, and resuming with a different
  file is rejected.
- **Two unsynchronized device clocks**: a capture device (A) and an
  analysis device (B) keep separate wall clocks with configurable skew
  and drift (`SFC_CLOCK_SKEW_MS`, `SFC_CLOCK_DRIFT_PPM`). Every event and
  cue record carries both timestamps; paired beacon samples are collected
  at chunk/session boundaries and a median-offset calibrator normalizes
  analysis timestamps into the capture clock domain. The calibration is
  exported in the report.
- **Intermediate state + resume + before/after comparison**: after every
  chunk a `checkpoint.json` is written atomically (tmp + rename) with a
  self-hash. On resume, the last `VERIFICATION_WINDOW` (8) cues processed
  before the pause are re-analyzed and their per-glyph result
  fingerprints are compared against the pre-pause hash; the outcome is
  recorded as `resume_comparison` in the report and as a
  `resume_verification` evidence event.
- **Resource insufficiency & degradation**: a `ResourceBudget` watches
  `MemAvailable` and per-chunk wall time. Under pressure the pipeline
  falls back from full Pango itemization (shaping + fallback fonts) to a
  cheap single-font charset probe; affected cues are marked
  `DegradedCheck` and a `DegradationEvent` (with reason) is logged.
  Test hooks: `SFC_FAKE_MEM_KB`, `SFC_FORCE_DEGRADE=1`.
- **Key evidence**: `evidence.jsonl` is an append-only, hash-chained
  event log (each event links `prev_hash`); the report stores the
  evidence file digest, so post-export tampering is detectable.
- **Field & order preservation on reload**: all artifacts are serialized
  from structs whose field declaration order is the JSON key order, with
  insertion-ordered maps. `verify` reloads `report.json`, re-serializes
  it, and requires byte-identical output.
- **Independent review**: a colleague runs
  `subtitle-font-coverage verify --dir <out>` to replay the evidence
  chain, recompute every digest, check the resume comparison, and confirm
  the media object hash. `report.md` is the human-readable twin.

## Usage

```sh
cargo build

# GUI (GTK4)
./target/debug/subtitle-font-coverage

# CLI
./target/debug/subtitle-font-coverage analyze <media-or-subtitle-file> [--out DIR] [--pause-after N]
./target/debug/subtitle-font-coverage resume  <media-or-subtitle-file> [--out DIR]
./target/debug/subtitle-font-coverage verify  [--dir DIR]
```

End-to-end demo (pause, degraded resume, verification):

```sh
scripts/demo.sh /tmp/sfc-demo
```

## Output artifacts

| file | content |
|---|---|
| `report.json` | ordered report: object id, clock calibration, sessions, degradation events, resume comparison, glyph coverage, per-cue records, digests |
| `report.md` | human-readable rendering of the same report |
| `checkpoint.json` | latest resumable intermediate state (self-hashed) |
| `evidence.jsonl` | hash-chained audit log of the whole run |

## Layout

- `src/extract.rs` — GStreamer `uridecodebin → appsink` cue extraction + Discoverer stream summary
- `src/coverage.rs` — Pango coverage engine (full itemization / degraded probe)
- `src/clocks.rs` — dual device clocks + offset calibrator
- `src/pipeline.rs` — chunked processing, pause/checkpoint/resume, verification window
- `src/evidence.rs` — hash-chained evidence log + chain verifier
- `src/report.rs` — report/Markdown export + independent `verify`
- `src/gui.rs` — GTK4 UI (file picker, start/pause/resume, progress, log)
