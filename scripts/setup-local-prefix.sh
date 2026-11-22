#!/usr/bin/env bash
# Reproducible no-root setup for hosts that lack GStreamer/GTK4 development
# packages. Downloads Ubuntu deb packages and extracts them into a user-local
# prefix; nothing is installed system-wide.
set -euo pipefail

PREFIX="${PREFIX:-$HOME/local/prefix}"
DEBS="${DEBS:-$HOME/local/debs}"
APT_OPTS=(-o Acquire::http::Proxy=DIRECT -o Acquire::https::Proxy=DIRECT)

GST_PACKAGES=(
  libgstreamer1.0-dev libgstreamer1.0-0
  libgstreamer-plugins-base1.0-dev libgstreamer-plugins-base1.0-0
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad
  libgstreamer-plugins-bad1.0-0 libgstreamer-gl1.0-0
  libgio-2.0-dev libglib2.0-0t64 libsysprof-capture-4-dev libpkgconf7
  gstreamer1.0-tools pkgconf
)

GTK_PACKAGES=(
  libgtk-4-dev libgtk-4-1 libgtk-4-common
  libcairo2-dev libpango1.0-dev libgdk-pixbuf-2.0-dev
  libgraphene-1.0-dev libharfbuzz-dev libepoxy-dev
  libvulkan-dev libwayland-dev libxkbcommon-dev
)

mkdir -p "$DEBS" "$PREFIX"
cd "$DEBS"
apt-get download "${APT_OPTS[@]}" "${GST_PACKAGES[@]}" "${GTK_PACKAGES[@]}"
for deb in *.deb; do
  dpkg-deb -x "$deb" "$PREFIX"
done

echo "Prefix ready at $PREFIX"
echo "Next: source scripts/local-env.sh, then cargo build --workspace"
