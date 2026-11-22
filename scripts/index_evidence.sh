#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

EVIDENCE="artifacts/evidence"
GENERATED_AT="$(date +%s%3N)"
UI_SCREENSHOT=""
if [ -f "$EVIDENCE/ui-smoke.png" ]; then
  UI_SCREENSHOT="ui-smoke.png"
fi
TEST_OUTPUTS="$(
  jq -n \
    --arg tests "$([ -f "$EVIDENCE/tests.txt" ] && echo tests.txt)" \
    --arg gstreamer "$([ -f "$EVIDENCE/gstreamer-test.txt" ] && echo gstreamer-test.txt)" \
    '[$tests, $gstreamer] | map(select(length > 0))'
)"
FILES_JSON="$(
  find "$EVIDENCE" -type f ! -name index.json -print0 \
    | sort -z \
    | xargs -0 sha256sum \
    | jq -R -s '
        split("\n")
        | map(select(length > 0))
        | map(capture("(?<sha256>[0-9a-f]{64})  (?<path>.*)"))
        | map({path: .path, sha256: .sha256})
      '
)"

jq -n \
  --argjson generated_at_unix_ms "$GENERATED_AT" \
  --slurpfile empty "$EVIDENCE/empty/evidence.json" \
  --slurpfile boundary "$EVIDENCE/boundary/evidence.json" \
  --slurpfile abnormal "$EVIDENCE/abnormal/evidence.json" \
  --slurpfile huge "$EVIDENCE/huge/evidence.json" \
  --slurpfile gstreamer "$EVIDENCE/gstreamer/evidence.json" \
  --arg ui_screenshot "$UI_SCREENSHOT" \
  --argjson test_outputs "$TEST_OUTPUTS" \
  --argjson files "$FILES_JSON" \
  '{
    schema_version: "encode-panel.index/v1",
    generated_at_unix_ms: $generated_at_unix_ms,
    toolchain: $boundary[0].toolchain,
    scenarios: {
      empty: {
        evidence: "empty/evidence.json",
        generated_samples: $empty[0].input.generated_samples,
        has_data: $empty[0].summary.has_data,
        waveform_points: ($empty[0].processing.waveform_points | length)
      },
      boundary: {
        evidence: "boundary/evidence.json",
        generated_samples: $boundary[0].input.generated_samples,
        boundary_events: $boundary[0].boundaries.events_total,
        before_after_captured: $boundary[0].coverage.boundary_before_after_captured,
        waveform_points: ($boundary[0].processing.waveform_points | length)
      },
      abnormal: {
        evidence: "abnormal/evidence.json",
        generated_samples: $abnormal[0].input.generated_samples,
        boundary_events: $abnormal[0].boundaries.events_total,
        retained_events: ($abnormal[0].boundaries.events | length),
        waveform_points: ($abnormal[0].processing.waveform_points | length)
      },
      huge: {
        evidence: "huge/evidence.json",
        requested_samples: $huge[0].input.requested_huge_samples,
        generated_samples: $huge[0].input.generated_samples,
        samples_processed: $huge[0].summary.samples,
        retained_events: ($huge[0].boundaries.events | length),
        waveform_points: ($huge[0].processing.waveform_points | length),
        duration_us: $huge[0].coverage.duration_us
      }
    },
    gstreamer: {
      evidence: "gstreamer/evidence.json",
      version: $gstreamer[0].report.gstreamer_version,
      samples_delivered: $gstreamer[0].report.samples_delivered,
      dropped_frames_detected: $gstreamer[0].report.dropped_frames_detected,
      bus_errors: ($gstreamer[0].report.bus_errors | length),
      waveform_points: ($gstreamer[0].waveform_points | length)
    },
    ui_screenshot: (if $ui_screenshot == "" then null else $ui_screenshot end),
    test_outputs: $test_outputs,
    files: $files
  }' > "$EVIDENCE/index.json"

echo "wrote $EVIDENCE/index.json"
