export SYSROOT=$HOME/sysroot
export PATH=$HOME/.cargo/bin:$SYSROOT/usr/bin:$PATH
export PKG_CONFIG=$SYSROOT/usr/bin/pkg-config
export PKG_CONFIG_PATH=$SYSROOT/usr/lib/x86_64-linux-gnu/pkgconfig:$SYSROOT/usr/share/pkgconfig
export PKG_CONFIG_SYSROOT_DIR=$SYSROOT
export LD_LIBRARY_PATH=$SYSROOT/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}
export GST_PLUGIN_PATH=$SYSROOT/usr/lib/x86_64-linux-gnu/gstreamer-1.0
export RUSTFLAGS="-L $SYSROOT/usr/lib/x86_64-linux-gnu"
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy
