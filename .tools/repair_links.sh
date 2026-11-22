#!/usr/bin/env bash
# Repair rootless-prefix shared library symlinks.
# - If the final target already exists under /usr/lib, point the prefix link there.
# - Otherwise download the providing runtime .deb and extract it into the prefix.
set -euo pipefail

TOOL_ROOT="$(cd "$(dirname "$0")" && pwd)"
PREFIX="$TOOL_ROOT/prefix"
LIBDIR="$PREFIX/usr/lib/x86_64-linux-gnu"

repair_one() {
  local link="$1"
  local target_name
  target_name="$(readlink "$link")"
  case "$target_name" in
    /*) return 0 ;;
  esac
  local dir
  dir="$(dirname "$link")"
  local final
  final="$(basename "$(readlink -f "$link" 2>/dev/null || echo "$link")")"
  if [ -e "/usr/lib/x86_64-linux-gnu/$final" ]; then
    ln -sfn "/usr/lib/x86_64-linux-gnu/$final" "$link"
  elif [ -e "$dir/$final" ]; then
    ln -sfn "$final" "$link"
  fi
}

while IFS= read -r link; do
  repair_one "$link"
done < <(find "$PREFIX" -xtype l -name '*.so*')

# Remaining broken links need runtime debs that the base system does not ship.
mapfile -t BROKEN < <(find "$PREFIX" -xtype l -name '*.so*')
if [ "${#BROKEN[@]}" -gt 0 ]; then
  NEEDED=$(printf '%s\n' "${BROKEN[@]}" | sed "s#^$PREFIX/usr/lib/x86_64-linux-gnu/##" | sort -u)
  echo "Still broken, resolving packages for:"
  echo "$NEEDED"
  PKGS=""
  while IFS= read -r soname; do
    pkg="$(dpkg -S "$soname" 2>/dev/null | head -1 | cut -d: -f1 || true)"
    if [ -z "$pkg" ]; then
      pkg="$(apt-cache search --names-only "$(echo "$soname" | sed 's/\.so.*//')" | awk '{print $1}' | grep -v -- '-dev' | head -1 || true)"
    fi
    [ -n "$pkg" ] && PKGS="$PKGS $pkg"
  done <<< "$NEEDED"
  PKGS="$(echo "$PKGS" | tr ' ' '\n' | sort -u | tr '\n' ' ')"
  echo "Downloading runtime packages:$PKGS"
  (
    cd "$TOOL_ROOT/debs"
    env -u http_proxy -u https_proxy -u all_proxy -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY \
      apt-get -o Acquire::http::Proxy=DIRECT -o Acquire::https::Proxy=DIRECT download $PKGS || true
    for deb in *.deb; do dpkg-deb -x "$deb" "$PREFIX/"; done
  )
fi

# Second pass now that runtime files may have landed.
while IFS= read -r link; do
  repair_one "$link"
done < <(find "$PREFIX" -xtype l -name '*.so*')

echo "Remaining broken symlinks: $(find "$PREFIX" -xtype l | wc -l)"
