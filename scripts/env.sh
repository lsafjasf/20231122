# Local toolchain environment (no root required).
# Usage: source scripts/env.sh
export SYSROOT="$HOME/sysroot/root"
export PATH="$HOME/.cargo/bin:$SYSROOT/usr/bin:$PATH"
export LD_LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PKG_CONFIG_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR="$SYSROOT"
export GST_PLUGIN_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
# The sandbox proxy at 127.0.0.1:7897 is dead; direct egress works.
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy
