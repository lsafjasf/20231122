#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$ROOT/assets/original/loudness_original.wav"
EXPECTED="6ae0c846d20bb1dec3590d39e326ecbb097359729b2c991c6f4c32c9a936d80e"

if [[ -f "$OUT" ]]; then
  ACTUAL="$(sha256sum "$OUT" | awk '{print $1}')"
  test "$ACTUAL" = "$EXPECTED"
  stat -c 'original unchanged: %n %s bytes mode=%a sha256='"$ACTUAL" "$OUT"
  exit 0
fi

mkdir -p "$(dirname "$OUT")"
gst-launch-1.0 -q concat name=c ! wavenc ! filesink location="$OUT" \
  audiotestsrc wave=sine freq=440 volume=0.08 num-buffers=5 samplesperbuffer=48000 \
    ! audio/x-raw,rate=48000,channels=1,format=S16LE ! c. \
  audiotestsrc wave=sine freq=440 volume=0.25 num-buffers=5 samplesperbuffer=48000 \
    ! audio/x-raw,rate=48000,channels=1,format=S16LE ! c. \
  audiotestsrc wave=sine freq=440 volume=0.015 num-buffers=5 samplesperbuffer=48000 \
    ! audio/x-raw,rate=48000,channels=1,format=S16LE ! c.

chmod 0444 "$OUT"
ACTUAL="$(sha256sum "$OUT" | awk '{print $1}')"
test "$ACTUAL" = "$EXPECTED"
echo "original generated: $OUT mode=0444 sha256=$ACTUAL"
