#!/usr/bin/env bash
# Source this file to configure the local GTK4/GStreamer sysroot and Rust
# toolchain for building submerge-gui on machines without system -dev packages.
export PATH="$HOME/.cargo/bin:$HOME/sysroot/usr/bin:$PATH"
export PKG_CONFIG_SYSROOT_DIR="$HOME/sysroot"
export PKG_CONFIG_PATH="$HOME/sysroot/usr/lib/x86_64-linux-gnu/pkgconfig:$HOME/sysroot/usr/share/pkgconfig"
export LD_LIBRARY_PATH="$HOME/sysroot/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
