# Source this file to expose the user-local dependency prefix (no root).
# shellcheck shell=bash

LAB_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SYSROOT="${LAB_ROOT}/../.sysroot"
SYSROOT="$(cd "$SYSROOT" && pwd)"

export PKG_CONFIG_SYSROOT_DIR="$SYSROOT"
export PKG_CONFIG_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export LD_LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu:$SYSROOT/usr/lib:${LD_LIBRARY_PATH:-}"
export PATH="$SYSROOT/usr/bin:$HOME/.cargo/bin:$PATH"

# GStreamer plugin/registry location inside the prefix.
export GST_PLUGIN_SYSTEM_PATH_1_0="$SYSROOT/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GST_PLUGIN_SCANNER="$SYSROOT/usr/lib/x86_64-linux-gnu/gstreamer-1.0/gst-plugin-scanner"
export GST_REGISTRY="$LAB_ROOT/target/gst-registry.bin"

# GIO modules (TLS etc.) and GTK settings.
export GIO_EXTRA_MODULES="$SYSROOT/usr/lib/x86_64-linux-gnu/gio/modules"
export GTK_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/gtk-4.0"
export XDG_DATA_DIRS="$SYSROOT/usr/share:${XDG_DATA_DIRS:-/usr/share}"

# Headless GTK4 runs use the Broadway HTML backend.
export GDK_BACKEND="${GDK_BACKEND:-broadway}"

# Cargo needs to bypass the dead local proxy when fetching crates.
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY all_proxy ALL_PROXY
export http_proxy="http://172.19.112.1:17897"
export https_proxy="http://172.19.112.1:17897"

echo "sysroot: $SYSROOT"
