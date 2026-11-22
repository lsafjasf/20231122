#!/usr/bin/env bash
# Build the three fixture containers used by the integration test:
#   normal.mp4       - healthy two-track MP4 (video + audio)
#   truncated.mp4    - normal.mp4 with its tail cut off
#   unknown.mp4      - normal.mp4 with a private top-level box ('zzzz')
set -euo pipefail

DIR="$(cd "$(dirname "$0")" && pwd)"
FIX="$DIR/../fixtures"
mkdir -p "$FIX"

FFMPEG="${FFMPEG:-ffmpeg}"

"$FFMPEG" -y -hide_banner -loglevel error \
    -f lavfi -i testsrc=size=160x120:rate=15:duration=2 \
    -f lavfi -i sine=frequency=440:duration=2 \
    -c:v libx264 -pix_fmt yuv420p -g 15 -c:a aac -shortest \
    -movflags +faststart \
    "$FIX/normal.mp4"

# Truncated: keep 70% of the bytes so the last top-level box is cut.
SIZE=$(stat -c %s "$FIX/normal.mp4")
KEEP=$(( SIZE * 70 / 100 ))
head -c "$KEEP" "$FIX/normal.mp4" > "$FIX/truncated.mp4"

# Unknown box: inject a well-formed private 'zzzz' box before moov.
python3 "$DIR/inject_unknown_box.py" \
    "$FIX/normal.mp4" "$FIX/unknown.mp4" zzzz

echo "fixtures written to $FIX"
ls -l "$FIX"
