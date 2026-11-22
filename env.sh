#!/usr/bin/env bash

USER_HOME="$(getent passwd "$(id -u)" | cut -d: -f6)"
export CARGO_HOME="${CARGO_HOME:-$USER_HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$USER_HOME/.rustup}"
export PATH="$CARGO_HOME/bin:$PATH"

echo "Rust: $(rustc --version 2>/dev/null || echo missing)"
echo "GStreamer: $(gst-launch-1.0 --version 2>/dev/null | head -n 1 || echo missing)"
echo "GTK4: $(pkg-config --modversion gtk4 2>/dev/null || echo missing)"
