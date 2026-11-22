# 开发/构建环境：本地 sysroot 中的 GTK4 + GStreamer 开发文件。
# 用法: source scripts/devenv.sh
export ROTFIX_SYSROOT="$HOME/.local/sysroot"
export PATH="$HOME/.cargo/bin:$ROTFIX_SYSROOT/usr/bin:$PATH"
export LD_LIBRARY_PATH="$ROTFIX_SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export PKG_CONFIG_SYSROOT_DIR="$ROTFIX_SYSROOT"
export PKG_CONFIG_LIBDIR="$ROTFIX_SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$ROTFIX_SYSROOT/usr/share/pkgconfig"
export GST_PLUGIN_PATH="$ROTFIX_SYSROOT/gst-plugins-rotfix"
export GST_PLUGIN_SYSTEM_PATH="$ROTFIX_SYSROOT/gst-plugins-rotfix"
# /tmp(tmpfs) 已满，构建临时文件放到工作区。
export TMPDIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/.tmp"
mkdir -p "$TMPDIR"
# 沙箱代理变量在此环境不可用，构建需直连。
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy
