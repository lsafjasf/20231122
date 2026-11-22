#!/usr/bin/env bash
set -euo pipefail

bundle="${1:?usage: verify-manifest.sh <evidence-bundle-dir>}"
manifest="$bundle/manifest.json"

test -f "$manifest" || {
  printf 'manifest not found: %s\n' "$manifest" >&2
  exit 1
}

jq -r '.files[] | "\(.sha256)  \(.path)"' "$manifest" |
  (cd "$bundle" && sha256sum -c -)

printf 'verified %s\n' "$bundle"
