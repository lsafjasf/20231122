# Source this file before cargo builds/runs in the offline sysroot.
export RUSTUP_HOME=/home/hj/.rustup
export CARGO_HOME=/home/hj/.cargo
source /home/hj/sysroot/env.sh
export PATH=/home/hj/.cargo/bin:$PATH
export GST_PLUGIN_SCANNER=/home/hj/sysroot/usr/libexec/gstreamer-1.0/gst-plugin-scanner
export CARGO_NET_OFFLINE=true
