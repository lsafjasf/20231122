# Mouse trajectory overlay rebuild

This repository rebuilds a screen-recording mouse-trail overlay with a fixed
Rust + GStreamer + GTK4 implementation. The original empty repository README is
left untouched. The two input materials are kept read-only under
`artifacts/original/`.

## Original material

| file | bytes | mode | SHA-256 |
| --- | ---: | ---: | --- |
| `artifacts/original/screen.mkv` | 6914135 | 0444 | `4156f32c8c96c912041c6d6dc3109eab521ce59391706c8abd34d53e31591fd3` |
| `artifacts/original/mouse-events.jsonl` | 3641 | 0444 | `1df374eb9ca980bac3e144af2c9b62fb5514fd3c6c1889148e19a2f0f50d9d11` |

`artifacts/evidence/original.sha256` is the independently stored checksum
manifest. The analysis records the hashes before and after the run and fails if
they change. The video is 320x180 RGB Matroska, 10 fps, 40 frames, exactly
4.000 seconds long.

The original video was generated once with:

```bash
gst-launch-1.0 -q videotestsrc num-buffers=40 pattern=smpte \
  ! video/x-raw,format=RGB,width=320,height=180,framerate=10/1 \
  ! matroskamux ! filesink location=artifacts/original/screen.mkv
```

## Processing chain

1. `filesrc ! decodebin` decodes the recording. Dynamic video pads are linked to
   `videoconvert ! video/x-raw,format=RGBA ! appsink`.
2. The event reader records the exact physical byte range of every JSONL record
   before decoding it. It rejects files over the configured byte limit before
   parsing, caps the record count, stable-sorts by `(ts_ns, raw_offset)`, and
   keeps the last physical record when timestamps are equal.
3. For each GStreamer sample, the sampler uses the sample PTS, duration and
   actual video dimensions. Exact events win; otherwise it interpolates when
   the surrounding event span is at most 500 ms, holds a stale position for at
   most 250 ms, then hides the cursor.
4. Coordinates are clamped to `[0, width-1] x [0, height-1]`. Every frame is
   written to `trace.jsonl` and `waveform.csv` with the selected event sequence,
   timestamp, raw offset, flags and parameter hash.
5. Rust draws the trail into the RGBA frame. GStreamer `appsrc` feeds
   `videoconvert ! video/x-raw,format=RGB ! matroskamux ! filesink`. Anchor
   frames are also encoded as PNG by GStreamer `pngenc`.
6. `waveform.svg` is the visual output waveform. `evidence.json` is the machine
   acceptance record. Raw anchor bytes are copied byte-for-byte to
   `artifacts/out/evidence/raw/anchor-{A,B,C}.jsonl`.

Processing parameters:

```text
track_id              = 0
frame rate            = 10/1
hold_ns               = 250000000
max_gap_ns            = 500000000
clamp_to_frame        = true
dedupe_policy         = same_ts_last_raw_offset_wins
interpolation         = linear_between_events_when_gap_lte_max
stale_policy          = hold_last_for_250ms_then_hide
params_sha256         = 870b276574b2798f0e494dc5dbefa90c67fc0e823139ce348386de74d69716e0
```

Every trace row and anchor repeats `params_sha256`, so a waveform point can be
joined back to the exact parameter set used to produce it.

## Three-anchor anomaly

The event log deliberately contains three anomalies. All timestamps below are
exact integer nanoseconds in the original JSONL file.

| anchor | time | track/frame | anomaly | raw offset | raw length | raw event | output |
| --- | ---: | --- | --- | ---: | ---: | --- | --- |
| A | `1000000000` ns | track 0, frame 10 | duplicate timestamp | 942 | 89 | seq 12, `(128,96)` | `(128,96)` |
| B | `2000000000` ns | track 0, frame 20 | raw-order inversion | 1931 | 90 | seq 23, `(200,105)` | `(200,105)` |
| C | `3000000000` ns | track 0, frame 30 | out-of-bounds clamp | 2741 | 90 | seq 32, `(365,-12)` | `(319,0)` |

At A, both records have offset 853 (`seq 11`, `(120,80)`) and offset 942
(`seq 12`, `(128,96)`). The dedupe rule selects the physically later record at
offset 942. The duplicate group is preserved in `evidence.json`.

At B, the `ts_ns=2000000000` record is physically after the
`ts_ns=2100000000` record. Reordering by timestamp selects offset 1931 and flags
`raw_order_inversion`. This is why the waveform at 2.000 s uses `(200,105)`,
while the next frame at 2.100 s uses `(210,110)`.

At C, the raw event is outside the 320x180 frame. The output is clamped to
`(319,0)` and carries `clamped_to_frame`, `clamp_x`, and `clamp_y`. The original
`(365,-12)` remains in the evidence.

The concrete acceptance command reports:

```text
PASS A@1.000s frame=10 raw_offset=942 raw_len=89 out=(128,96) |
B@2.000s frame=20 raw_offset=1931 raw_len=90 out=(200,105) |
C@3.000s frame=30 raw_offset=2741 raw_len=90 out=(319,0)
```

## Empty and oversized input

- Empty event file plus a valid video: status `ok_empty_events`; all 40 frame
  samples are hidden with `stale_hidden`, and the overlay/trace/waveform are
  still produced without panic.
- Empty video file: status `error`, code `E_EMPTY_VIDEO`, raw offset 0. The
  diagnostic is written to `evidence.json` before exiting with code 2.
- Event input over `--max-event-bytes` (default 128 MiB): rejected from file
  metadata before parsing, code `E_EVENT_BYTES_LIMIT`. The diagnostic records
  the configured byte boundary as the raw offset.
- Event count over `--max-events` (default 1000000): code
  `E_EVENT_COUNT_LIMIT`. The diagnostic records the raw offset of the first
  over-limit record. The 41-record fixture with `--max-events 10` reports offset
  853.
- Video over 1000000 decoded frames: bounded pipeline error
  `E_VIDEO_PIPELINE`; the run does not grow an unbounded trace in memory.
- A 250000-record, 20580656-byte stress log completes successfully in 0.23 s
  with 186316 KiB peak RSS on this machine, producing three anchors.

## Run and verify

Prerequisites are Rust, GStreamer 1.28 with `decodebin`, `videoconvert`,
`appsink`, `appsrc`, `matroskamux`, `pngenc`, and GTK4 4.8 or newer.

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo test
cargo run --release -- analyze \
  --video artifacts/original/screen.mkv \
  --events artifacts/original/mouse-events.jsonl \
  --out artifacts/out
cargo run --release -- verify --out artifacts/out
cargo run --release -- gui --out artifacts/out
```

`scripts/reproduce.sh` runs the checksum check, tests, analysis and acceptance
in order. `scripts/stress-events.sh [count]` reproduces the large-input run.

The GTK4 inspector shows the three anchor frames, the full waveform, a frame
cursor, raw offsets, raw JSON, parameter hash and flags. It deliberately does
not modify any output or original material.

## Evidence map

| artifact | purpose |
| --- | --- |
| `artifacts/evidence/original.sha256` | immutable input checksums |
| `artifacts/out/evidence.json` | acceptance record, params, input/output hashes, anchors, counts |
| `artifacts/out/evidence/anchors.json` | anchor-only evidence detail |
| `artifacts/out/evidence/raw/anchor-*.jsonl` | exact original bytes at each anchor |
| `artifacts/out/trace.jsonl` | per-frame waveform and provenance |
| `artifacts/out/waveform.csv` | tabular waveform |
| `artifacts/out/waveform.svg` | visual waveform with anchor lines |
| `artifacts/out/overlay.mkv` | 4-second annotated video |
| `artifacts/out/frames/anchor-*.png` | GTK4-inspector anchor stills |
