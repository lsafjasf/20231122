# Source this file to build/run the svbatch experiment.
# Usage: source ./env.sh
export RUSTUP_HOME=/home/hj/.rustup
export CARGO_HOME=${CARGO_HOME:-$HOME/.cargo-svbatch}
export PATH=/home/hj/.cargo/bin:$PATH
export SVBATCH_ENV_ROOT=/home/hj/env-media
export PKG_CONFIG_PATH=$SVBATCH_ENV_ROOT/lib/pkgconfig
export LD_LIBRARY_PATH=$SVBATCH_ENV_ROOT/lib:${LD_LIBRARY_PATH:-}
export GST_PLUGIN_SYSTEM_PATH=$SVBATCH_ENV_ROOT/lib/gstreamer-1.0
export GST_PLUGIN_SCANNER=$SVBATCH_ENV_ROOT/libexec/gstreamer-1.0/gst-plugin-scanner
export PATH=$SVBATCH_ENV_ROOT/bin:$PATH
# The host-wide proxy on :7897 is not listening; crates.io is reachable directly.
unset http_proxy https_proxy all_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY
