# Source this file when building against a user-local GStreamer/GTK4 prefix
# (for hosts without root access). It expects the extracted deb layout under
# $HOME/local/prefix produced by scripts/setup-local-prefix.sh.

LOCAL_PREFIX="${LOCAL_PREFIX:-$HOME/local/prefix}"
MULTIARCH="x86_64-linux-gnu"

export PATH="$HOME/.cargo/bin:$LOCAL_PREFIX/usr/bin:$PATH"
export PKG_CONFIG_SYSROOT_DIR="$LOCAL_PREFIX"
export PKG_CONFIG_PATH="$LOCAL_PREFIX/usr/lib/$MULTIARCH/pkgconfig:$LOCAL_PREFIX/usr/share/pkgconfig"
export PKG_CONFIG_SYSTEM_INCLUDE_PATH=""
export PKG_CONFIG_SYSTEM_LIBRARY_PATH=""
export LD_LIBRARY_PATH="$LOCAL_PREFIX/usr/lib/$MULTIARCH:$LOCAL_PREFIX/lib/$MULTIARCH:$LD_LIBRARY_PATH"
export GST_PLUGIN_PATH="$LOCAL_PREFIX/usr/lib/$MULTIARCH/gstreamer-1.0"
export GST_PLUGIN_SYSTEM_PATH="$LOCAL_PREFIX/usr/lib/$MULTIARCH/gstreamer-1.0"
export GST_REGISTRY="$HOME/local/gst-registry.bin"
export XDG_DATA_DIRS="$LOCAL_PREFIX/usr/share:$XDG_DATA_DIRS"

# The sandbox in this environment ships a dead proxy configuration; cargo and
# apt only work once these are unset. Harmless on normally configured hosts.
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy 2>/dev/null || true
