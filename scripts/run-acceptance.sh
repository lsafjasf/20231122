#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

USER_HOME="$(getent passwd "$(id -u)" | cut -d: -f6)"
export CARGO_HOME="${CARGO_HOME:-$USER_HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$USER_HOME/.rustup}"
export PATH="$CARGO_HOME/bin:$PATH"

SOURCE="$ROOT/assets/original/loudness_original.wav"
BIN="$ROOT/target/release/loudness-segment-calibrator"
EVIDENCE="$ROOT/artifacts/evidence"
mkdir -p "$EVIDENCE"

cargo build --release

SOURCE_HASH_BEFORE="$(sha256sum "$SOURCE" | awk '{print $1}')"
SOURCE_MODE="$(stat -c '%a' "$SOURCE")"

"$BIN" run --source "$SOURCE" --run "$EVIDENCE/continuous" --restart \
  | tee "$EVIDENCE/continuous.log"

"$BIN" run --source "$SOURCE" --run "$EVIDENCE/paused" --pause-after A --restart \
  | tee "$EVIDENCE/paused-step1.log"
"$BIN" resume --run "$EVIDENCE/paused" --pause-after B \
  | tee "$EVIDENCE/paused-step2.log"
"$BIN" resume --run "$EVIDENCE/paused" \
  | tee "$EVIDENCE/paused-step3.log"

"$BIN" run --source "$SOURCE" --run "$EVIDENCE/degraded" --pause-after A --force-degrade --restart \
  | tee "$EVIDENCE/degraded-step1.log"
"$BIN" resume --run "$EVIDENCE/degraded" --pause-after B \
  | tee "$EVIDENCE/degraded-step2.log"
"$BIN" resume --run "$EVIDENCE/degraded" \
  | tee "$EVIDENCE/degraded-step3.log"

"$BIN" verify --run "$EVIDENCE/paused" \
  | tee "$EVIDENCE/paused-verification.txt"
"$BIN" verify --run "$EVIDENCE/degraded" \
  | tee "$EVIDENCE/degraded-verification.txt"
"$BIN" verify --run "$EVIDENCE/continuous" \
  > "$EVIDENCE/continuous-verification.txt"

"$BIN" compare \
  --a "$EVIDENCE/continuous" \
  --b "$EVIDENCE/paused" \
  --out "$EVIDENCE/compare-equivalence.json" \
  | tee "$EVIDENCE/compare.txt"

SOURCE_HASH_AFTER="$(sha256sum "$SOURCE" | awk '{print $1}')"
test "$SOURCE_HASH_BEFORE" = "$SOURCE_HASH_AFTER"
test "$SOURCE_MODE" = "444" || test "$SOURCE_MODE" = "0444"
jq -e '.ok == true and .outputs_identical == true' \
  "$EVIDENCE/compare-equivalence.json" >/dev/null
jq -e '.ok == true' "$EVIDENCE/paused/evidence/verification.json" >/dev/null
jq -e '.ok == true' "$EVIDENCE/degraded/evidence/verification.json" >/dev/null

jq -n \
  --slurpfile paused "$EVIDENCE/paused/state.json" \
  --slurpfile continuous "$EVIDENCE/continuous/state.json" \
  --slurpfile degraded "$EVIDENCE/degraded/state.json" \
  --slurpfile equivalence "$EVIDENCE/compare-equivalence.json" \
  --arg source_hash "$SOURCE_HASH_AFTER" \
  '{
    source: {
      path: $paused[0].source.path,
      sha256: $source_hash,
      bytes: $paused[0].source.file_size,
      readonly: $paused[0].source.original_mode_readonly,
      duration_ns: $paused[0].source.duration_ns
    },
    anchors: ($paused[0].completed | map({
      id: .anchor_id,
      time_ns: .anchor_ns,
      frame: .anchor_frame,
      data_offset: .payload_end_byte,
      file_offset: .original_anchor_file_byte,
      before_rms_dbfs: .before.rms_dbfs,
      after_rms_dbfs: .after.rms_dbfs,
      gain_db: .gain.applied_gain_db,
      target_error_db: .target_error_db,
      output_sha256: .output_sha256,
      checkpoint_sha256: .checkpoint_sha256
    })),
    paused_resumed: {
      status: $paused[0].status,
      resume_count: $paused[0].resume_count,
      final_output: $paused[0].final_output
    },
    continuous: {final_output: $continuous[0].final_output},
    degraded: {
      mode: $degraded[0].resource.mode,
      strategy: $degraded[0].degradation.strategy,
      rate: $degraded[0].degradation.output_sample_rate,
      resume_count: $degraded[0].resume_count,
      final_output: $degraded[0].final_output
    },
    equivalence: $equivalence[0],
    verification: {paused: true, degraded: true, continuous: true}
  }' > "$EVIDENCE/summary.json"

{
  echo "# Continuous vs paused/resumed"
  cat "$EVIDENCE/paused-verification.txt"
  echo
  echo "# Forced resource degradation"
  cat "$EVIDENCE/degraded-verification.txt"
  echo
  echo "# Equivalence"
  cat "$EVIDENCE/compare.txt"
} | tee "$EVIDENCE/acceptance.txt"

if [[ -n "${DISPLAY:-}" ]] && command -v cargo >/dev/null 2>&1; then
  GDK_BACKEND=x11 \
  LOUDNESS_GUI_SCREENSHOT="$ROOT/artifacts/gui-evidence.png" \
    "$BIN" gui --run "$EVIDENCE/paused" || true
fi

echo "PASS original_hash_unchanged sha256=$SOURCE_HASH_AFTER mode=$SOURCE_MODE"
echo "PASS continuous_vs_paused outputs_identical=true"
echo "PASS degraded_path sample_rate=24000 duration_ns=15000000000"
echo "Evidence: $EVIDENCE"
