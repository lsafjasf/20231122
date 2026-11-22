#!/usr/bin/env bash
# 把预置 deb 集合解包到本地前缀（无需 root），并修正 pkg-config 前缀与悬空开发链接。
# 用法：DEB_DIR=/tmp/silence-debs-0052 bash scripts/setup-local-deps.sh
set -euo pipefail

DEB_DIR="${DEB_DIR:?请设置 DEB_DIR 指向 deb 集合目录}"
LOCAL_PREFIX="${LOCAL_PREFIX:-$HOME/local}"

mkdir -p "$LOCAL_PREFIX"
for deb in "$DEB_DIR"/*.deb; do
  dpkg-deb -x "$deb" "$LOCAL_PREFIX"
done

# deb 内 .pc 的 prefix 指向 /usr，改写为本地前缀。
for pc in "$LOCAL_PREFIX"/usr/lib/x86_64-linux-gnu/pkgconfig/*.pc; do
  sed -i "s|^prefix=/usr\$|prefix=$LOCAL_PREFIX/usr|" "$pc"
done

# 开发符号链接可能指向仅存在于系统目录的版本化运行库，补全悬空链接。
cd "$LOCAL_PREFIX/usr/lib/x86_64-linux-gnu"
for link in $(find . -maxdepth 1 -type l -name '*.so'); do
  target=$(readlink "$link")
  if [ ! -e "$link" ] && [ -e "/usr/lib/x86_64-linux-gnu/$target" ]; then
    ln -sf "/usr/lib/x86_64-linux-gnu/$target" "$link"
  fi
done

echo "依赖已解包到 $LOCAL_PREFIX，执行: source scripts/env.local.sh"
