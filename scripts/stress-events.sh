#!/usr/bin/env bash
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${PATH}"
cd "$(dirname "$0")/.."

count="${1:-250000}"
events="$(mktemp /tmp/mouse-trail-stress-events.XXXXXX.jsonl)"
out="$(mktemp -d /tmp/mouse-trail-stress-out.XXXXXX)"
trap 'rm -f "${events}"' EXIT

awk -v count="${count}" 'BEGIN {
  for (i = 1; i <= count; i++) {
    printf "{\"seq\":%d,\"ts_ns\":%d,\"x\":%.1f,\"y\":%.1f,\"buttons\":0,\"source\":\"stress\"}\n",
      i, (i - 1) * 16000, (i - 1) % 320, (i - 1) % 180
  }
}' > "${events}"

echo "events=${count} bytes=$(stat -c %s "${events}")"
/usr/bin/time -f "elapsed_s=%e maxrss_kib=%M" \
  target/release/mouse-trail-rebuild analyze \
  --video artifacts/original/screen.mkv \
  --events "${events}" \
  --out "${out}" \
  --max-events 1000000
jq '{status,counts,anchors:(.anchors|length),source_unchanged,event_bytes:.inputs.events.bytes}' \
  "${out}/evidence.json"
echo "out=${out}"
