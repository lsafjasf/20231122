#!/bin/bash
# 本地工具链环境（rustc/cargo 未系统安装时）
export PATH=/tmp/rustroot/usr/lib/rust-1.93/bin:$PATH
export LD_LIBRARY_PATH=/tmp/rustroot/usr/lib/x86_64-linux-gnu:/tmp/rustroot/usr/lib/rust-1.93/lib
exec "$@"
