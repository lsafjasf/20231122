#!/usr/bin/env bash
# Headless GTK4 verification: start Xvfb, launch the GUI, capture a
# screenshot, then tear everything down. No root required.
set -euo pipefail

LAB_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SYSROOT="$(cd "$LAB_ROOT/../.sysroot" && pwd)"
DISPLAY_NUM="${DISPLAY_NUM:-124}"
SHOT="$LAB_ROOT/evidence/screenshots/gtk4-gui.png"

# shellcheck source=env.sh
source "$LAB_ROOT/scripts/env.sh"
export GDK_BACKEND=x11
export GTK_A11Y=none
export GSETTINGS_SCHEMA_DIR="$SYSROOT/usr/share/glib-2.0/schemas"
export DISPLAY=":$DISPLAY_NUM"

if ! pgrep -f "Xvfb :$DISPLAY_NUM" >/dev/null; then
  DISPLAY_NUM="$DISPLAY_NUM" setsid nohup "$LAB_ROOT/scripts/start-xvfb.sh" \
    >/tmp/hls-xvfb.log 2>&1 < /dev/null &
  sleep 5
fi

cd "$LAB_ROOT"
setsid nohup ./target/debug/hls-lab gui >/tmp/hls-gui.log 2>&1 < /dev/null &
GUI_PID=$!
sleep 4

mkdir -p "$(dirname "$SHOT")"
"$SYSROOT/usr/bin/xwd" -root -silent -out /tmp/hls-gui.xwd
ffmpeg -hide_banner -loglevel error -y -i /tmp/hls-gui.xwd "$SHOT"

kill "$GUI_PID" 2>/dev/null || true
echo "screenshot saved: $SHOT"
