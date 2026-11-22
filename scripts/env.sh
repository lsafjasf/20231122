#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REAL_HOME="$(getent passwd "$(id -u)" 2>/dev/null | cut -d: -f6 || true)"
REAL_HOME="${REAL_HOME:-$HOME}"
export PATH="$REAL_HOME/.cargo/bin:$HOME/.cargo/bin:$PATH"

if [[ -d "$REAL_HOME/env-media" ]]; then
  export PATH="$REAL_HOME/env-media/bin:$PATH"
  export PKG_CONFIG_PATH="$REAL_HOME/env-media/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  export LD_LIBRARY_PATH="$REAL_HOME/env-media/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
  export GST_PLUGIN_SYSTEM_PATH="$REAL_HOME/env-media/lib/gstreamer-1.0${GST_PLUGIN_SYSTEM_PATH:+:$GST_PLUGIN_SYSTEM_PATH}"
fi

export BWF_REPLAY_ROOT="$ROOT"

