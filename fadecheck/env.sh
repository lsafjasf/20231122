# 源自我行: source env.sh
# 用户态前缀（无 root 安装的 GTK4 / GStreamer 开发库与插件）
export FADECHECK_PREFIX="$HOME/local/sys/usr"
export PATH="$FADECHECK_PREFIX/bin:$HOME/.cargo/bin:$PATH"
export LD_LIBRARY_PATH="$FADECHECK_PREFIX/lib/x86_64-linux-gnu:${LD_LIBRARY_PATH:-}"
export LIBRARY_PATH="$FADECHECK_PREFIX/lib/x86_64-linux-gnu:${LIBRARY_PATH:-}"
export C_INCLUDE_PATH="$FADECHECK_PREFIX/include:${C_INCLUDE_PATH:-}"
export PKG_CONFIG_PATH="$FADECHECK_PREFIX/lib/x86_64-linux-gnu/pkgconfig:$FADECHECK_PREFIX/share/pkgconfig:${PKG_CONFIG_PATH:-}"
export GST_PLUGIN_SYSTEM_PATH_1_0="$FADECHECK_PREFIX/lib/x86_64-linux-gnu/gstreamer-1.0:/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
# rust-lld 不读取 LIBRARY_PATH，需通过 RUSTFLAGS 显式给出本地库搜索路径
export RUSTFLAGS="-L native=$FADECHECK_PREFIX/lib/x86_64-linux-gnu ${RUSTFLAGS:-}"
unset http_proxy https_proxy all_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY
