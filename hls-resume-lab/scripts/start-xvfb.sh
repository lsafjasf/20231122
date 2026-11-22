#!/usr/bin/env bash
# Start Xvfb headlessly without root. Xvfb hardcodes /usr/bin/xkbcomp, which
# is absent on this host. bwrap's tmp-overlay makes /usr/bin temporarily
# writable inside a user namespace so the sysroot copy can be bind-mounted
# onto that exact path.
set -euo pipefail

LAB_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SYSROOT="$(cd "$LAB_ROOT/../.sysroot" && pwd)"
DISPLAY_NUM="${DISPLAY_NUM:-123}"

export LD_LIBRARY_PATH="$SYSROOT/usr/lib/x86_64-linux-gnu:$SYSROOT/usr/lib"

exec 3< "$SYSROOT/usr/bin/xkbcomp"
bwrap \
  --unshare-user \
  --ro-bind / / \
  --dev /dev \
  --proc /proc \
  --bind /tmp /tmp \
  --overlay-src /usr/bin \
  --tmp-overlay /usr/bin \
  --perms 0755 \
  --ro-bind-data 3 /usr/bin/xkbcomp \
  --setenv LD_LIBRARY_PATH "$LD_LIBRARY_PATH" \
  --chdir "$LAB_ROOT" \
  -- "$SYSROOT/usr/bin/Xvfb" ":$DISPLAY_NUM" \
     -screen 0 1024x768x24 -nolisten tcp
