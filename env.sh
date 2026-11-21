# 本地 sysroot 环境（无 sudo 场景）。用法: source env.sh
export SYSROOT="$HOME/sysroot"
export PATH="$SYSROOT/usr/bin:$HOME/.cargo/bin:$PATH"
export LD_LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PKG_CONFIG_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig"
export GST_PLUGIN_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
