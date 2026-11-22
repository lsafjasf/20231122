# Local (rootless) toolchain environment for the segment-fingerprint project.
# Sourced by build/test commands; no system packages are modified.
TOOL_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
PREFIX="$TOOL_ROOT/prefix"

export PATH="$HOME/.cargo/bin:$PREFIX/usr/bin:$PATH"
export LD_LIBRARY_PATH="$PREFIX/usr/lib/x86_64-linux-gnu:$PREFIX/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PKG_CONFIG="$PREFIX/usr/bin/pkg-config"
export PKG_CONFIG_PATH="$PREFIX/usr/lib/x86_64-linux-gnu/pkgconfig:$PREFIX/usr/share/pkgconfig"
export PKG_CONFIG_ALLOW_SYSTEM_LIBS=1
export PKG_CONFIG_ALLOW_SYSTEM_CFLAGS=1
# Keep both the rootless prefix and the distro plugin directory: core elements
# such as typefind ship in the system gstreamer1.0 package.
export GST_PLUGIN_PATH="$PREFIX/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GST_PLUGIN_SYSTEM_PATH="$PREFIX/usr/lib/x86_64-linux-gnu/gstreamer-1.0:/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GIO_EXTRA_MODULES="$PREFIX/usr/lib/x86_64-linux-gnu/gio/modules"
export XDG_DATA_DIRS="$PREFIX/usr/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"

# Make the rootless prefix visible to the rust-lld linker as well as pkg-config.
export RUSTFLAGS="-L $PREFIX/usr/lib/x86_64-linux-gnu ${RUSTFLAGS:-}"
