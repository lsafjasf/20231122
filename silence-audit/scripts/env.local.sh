# 本地无 root 构建环境配置：source 本文件后再执行 cargo。
# 依赖预置 deb 已解包到 $LOCAL_PREFIX（见 scripts/setup-local-deps.sh）。

export LOCAL_PREFIX="${LOCAL_PREFIX:-$HOME/local}"
export PREFIX="$LOCAL_PREFIX/usr"
export HOME="${REAL_HOME:-$HOME}"
export PATH="$PREFIX/bin:$HOME/.cargo/bin:$PATH"
export LD_LIBRARY_PATH="$PREFIX/lib/x86_64-linux-gnu:$PREFIX/lib:/usr/lib/x86_64-linux-gnu"
export PKG_CONFIG_PATH="$PREFIX/lib/x86_64-linux-gnu/pkgconfig:$PREFIX/share/pkgconfig"
export PKG_CONFIG_SYSROOT_DIR=/
export GST_PLUGIN_SYSTEM_PATH="$PREFIX/lib/x86_64-linux-gnu/gstreamer-1.0:/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GIO_EXTRA_MODULES="$PREFIX/lib/x86_64-linux-gnu/gio/modules"
export CARGO_NET_OFFLINE=true
