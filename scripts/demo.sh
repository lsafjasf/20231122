#!/usr/bin/env bash
# End-to-end demonstration of vkri using synthetic GStreamer clips.
# Requires: gst-launch-1.0 (vp8/webm), the built `vkri` binary, sha256sum.
set -euo pipefail

VKRI="${VKRI:-target/debug/vkri}"
WORK="${WORK:-/tmp/vkri-demo}"
SEED_BUF="${SEED_BUF:-120}"          # 4s at 30fps
ORIGIN=1700000000000000000
OFFSET_B=1500000000                  # device B clock runs 1.5s ahead

rm -rf "$WORK"
mkdir -p "$WORK/media" "$WORK/exports"
PROJ="$WORK/project"

gst-launch-1.0 -q videotestsrc pattern=ball num-buffers="$SEED_BUF" \
  ! video/x-raw,framerate=30/1 ! vp8enc cpu-used=4 deadline=1 ! webmmux \
  ! filesink location="$WORK/media/a.webm"
gst-launch-1.0 -q videotestsrc pattern=smpte num-buffers="$SEED_BUF" \
  ! video/x-raw,framerate=30/1 ! vp8enc cpu-used=4 deadline=1 ! webmmux \
  ! filesink location="$WORK/media/b.webm"

"$VKRI" init "$PROJ" --name demo --description "two clock-skewed devices, one object"
DA=$("$VKRI" add-device "$PROJ" --label camera-A | sed -E 's/.*id=([^ ]+).*/\1/')
DB=$("$VKRI" add-device "$PROJ" --label camera-B | sed -E 's/.*id=([^ ]+).*/\1/')
"$VKRI" set-offset "$PROJ" --device-id "$DB" --offset-ns "$OFFSET_B" --uncertainty-ns 50000000

SA=$("$VKRI" register "$PROJ" --device-id "$DA" --label frag-A \
  --source "$WORK/media/a.webm" --clock-origin-unix-ns "$ORIGIN" \
  --group object_7 --mode interval --interval-ns 1000000000 --thumbnail-max-edge 160 \
  | sed -E 's/.*id=([^ ]+).*/\1/')
SB=$("$VKRI" register "$PROJ" --device-id "$DB" --label frag-B \
  --source "$WORK/media/b.webm" --clock-origin-unix-ns "$ORIGIN" \
  --group object_7 --mode interval --interval-ns 1000000000 --thumbnail-max-edge 160 \
  | sed -E 's/.*id=([^ ]+).*/\1/')

"$VKRI" run "$PROJ" --segment-id "$SA"
"$VKRI" run "$PROJ" --segment-id "$SB"
"$VKRI" compare "$PROJ" --segment-a "$SA" --segment-b "$SB" --match-window-ns 900000000
"$VKRI" export "$PROJ" --export-root "$WORK/exports" --bundle demo-bundle
"$VKRI" verify-export "$WORK/exports/demo-bundle"

echo
echo "Independent review without vkri:"
echo "  (cd $WORK/exports/demo-bundle && sha256sum -c SHA256SUMS)"
