# Source this for running produced binaries outside of cargo.
export SYSROOT=/home/hj/.codex-relay-home-jdfvng3h/sysroot/root
export PKG_CONFIG="$SYSROOT/usr/bin/pkg-config"
export PKG_CONFIG_SYSROOT_DIR="$SYSROOT"
export PKG_CONFIG_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export LD_LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export GST_PLUGIN_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GST_PLUGIN_SYSTEM_PATH=
