# VFR Replay Lab — 变帧率剪辑预览的可回放实验

A replayable variable-frame-rate (VFR) clip-preview experiment built with the
fixed stack **Rust + GStreamer 1.28 + GTK 4.22**.

Every experiment step records its **input**, the **state before/after**, and
the **result** in an append-only, SHA-256-chained journal. The journal is then
replayed from a recovered genesis state, including an *unknown extension*
control, an *order change*, and a *duplicate submission*.

## Layered architecture

| Layer | File | Native deps | Responsibility |
| --- | --- | --- | --- |
| Pure core | `src/model.rs`, `src/engine.rs`, `src/journal.rs`, `src/replay.rs`, `src/persist.rs` | none | commands, state, hash chain, deterministic replay |
| GStreamer | `src/media.rs` | gstreamer/app/video 1.28 | VFR fixture synthesis, PTS probing, rate-normalized preview |
| GTK4 shell | `src/appui.rs`, `src/bin/gtk.rs` | GTK 4.22 | renders the replayed journal; headless via Broadway |
| Driver | `src/bin/lab.rs` | GStreamer | scenario runner, replay, tamper control |

Features are additive: `--no-default-features` builds/test the pure core with
no native libraries; `--features gst` adds media; `--features gtk` adds the GUI.

## Environment (no root required)

The machine has no system Rust/GTK dev packages and a dead proxy on
`127.0.0.1:7897`. Dependencies were installed user-locally:

- Rust toolchain: `~/.cargo/bin` (rustup, direct network with proxy vars unset).
- Native sysroot: extracted `.deb` packages under `/home/hj/local`
  (GStreamer, plugins, GTK4, broadwayd).

Activate with:

```bash
source /home/hj/local/env.sh
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY
```

## Run the whole experiment

```bash
cd vfr-replay-lab
./run_experiment.sh
```

This builds all layers, runs 13 tests, executes the recorded scenario, replays
the recovered journal, proves tamper detection, and renders the GTK4 view to a
PNG under `../evidence/run/`.

## Manual commands

```bash
# Pure-core tests only (no GStreamer/GTK needed)
cargo test --no-default-features

# Full test suite
cargo test --features gtk

# Individual driver commands
./target/debug/vfr-lab fixture /tmp/clip.webm
./target/debug/vfr-lab run-scenario ../evidence/run
./target/debug/vfr-lab replay  ../evidence/run/journal.jsonl
./target/debug/vfr-lab tamper  ../evidence/run/journal.jsonl

# Headless GTK4 window (Broadway daemon must be running)
gtk4-broadwayd -p 8097 :7 &
GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 \
  ./target/debug/vfr-gtk ../evidence/run/journal.jsonl /tmp/shot.png
```

## Recorded experiment flow

1. `AddClip` a genuinely VFR WebM (irregular PTS synthesized through
   `appsrc → videoconvert → vp8enc → webmmux`).
2. `Place` it and `SetRate(24/1)`; run a `videorate` preview and capture PTS.
3. Add a second clip, place it, then `Swap(0,1)` — the **order change**.
4. `SetRate(60/1)` and re-preview; output PTS become a constant 1/60 s grid.
5. Submit the same `Remove` twice with one idempotency key — the
   **duplicate submission**; the second returns `Duplicate` and changes nothing.
6. Send `UnknownExtension("x-experimental-twist")` — the **control sample**;
   recorded as `UnknownIgnored`, state stays semantically unchanged.
7. Recover the journal from disk and replay all 10 inputs; states, outcomes and
   the full hash chain must match.

## Evidence

See `../evidence/run/` and the machine-readable `evidence.json`. Tamper
detection is demonstrated on `journal.tampered.jsonl` (replay exits non-zero).
