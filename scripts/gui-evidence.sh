#!/bin/sh
# Capture GTK4 GUI evidence headlessly: cage (wlroots headless backend)
# runs the app, grim takes the screenshot via wlr-screencopy.
set -eu
cd "$(dirname "$0")/.."

# shellcheck disable=SC1091
. "${SYSROOT_ENV:-$HOME/sysroot/env.sh}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-$HOME/xdg-runtime}"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
find "$XDG_RUNTIME_DIR" -name 'wayland-*' -delete 2>/dev/null || true
export XKB_CONFIG_ROOT="${XKB_CONFIG_ROOT:-$HOME/sysroot/usr/share/X11/xkb}"
FC_DIR="$HOME/.cache/ditherdiag-fontconfig"
mkdir -p "$FC_DIR/cache"
cat > "$FC_DIR/fonts.conf" <<EOF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>$HOME/sysroot/usr/share/fonts</dir>
  <cachedir>$FC_DIR/cache</cachedir>
</fontconfig>
EOF
export FONTCONFIG_FILE="${FONTCONFIG_FILE:-$FC_DIR/fonts.conf}"
export XDG_DATA_DIRS="${XDG_DATA_DIRS:-$HOME/sysroot/usr/share}"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1

OUT="${1:-artifacts/gui-evidence.png}"

cage -s -- sleep 90 &
CAGE_PID=$!
trap 'kill "$CAGE_PID" 2>/dev/null || true' EXIT
sleep 4
SOCK=$(ls "$XDG_RUNTIME_DIR" | grep '^wayland-[0-9]' | head -1)
echo "compositor socket: $SOCK"

WAYLAND_DISPLAY=$SOCK GDK_BACKEND=wayland GSK_RENDERER=cairo DITHERDIAG_AUTO=1 \
    ./target/debug/ditherdiag gui &
GUI_PID=$!
# The auto-run experiment takes ~15 s; let the UI reach its final state.
sleep 25
WAYLAND_DISPLAY=$SOCK grim "$OUT"
kill "$GUI_PID" 2>/dev/null || true
echo "wrote $OUT"
