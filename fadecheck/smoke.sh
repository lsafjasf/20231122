#!/usr/bin/env bash
# 端到端冒烟：生成夹具 → 无头批处理 → 校验判定与临时目录清理。
# 用法: source env.sh && ./smoke.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
FIX="${TMPDIR:-/tmp}/fadecheck-smoke-$$"
mkdir -p "$FIX"
trap 'rm -rf "$FIX"' EXIT

cargo build
python3 "$ROOT/tests/make_fixtures.py" "$FIX"
cp "$FIX/fade_both.wav" "$FIX/mystery.xyz"

REPORT="$FIX/report.json"
./target/debug/fadecheck --batch \
  "$FIX/mystery.xyz" \
  "$FIX/boundary.wav" \
  "$FIX/fade_both.wav" \
  "$FIX/no_fade.wav" \
  "$FIX/missing.wav" \
  > "$REPORT"

echo "== 判定结果 =="
jq -r '.conclusions[] | "#\(.object_id) \(.verdict)  \(.path)"' "$REPORT"

EXPECTED='["NotAnalyzable","Inconclusive","FadeInAndOut","NoFade","NotAnalyzable"]'
ACTUAL="$(jq -c '[.conclusions[].verdict]' "$REPORT")"
test "$ACTUAL" = "$EXPECTED"

# 临时状态必须在进程结束时清理干净
if compgen -G "${TMPDIR:-/tmp}/fadecheck-batch-*" > /dev/null; then
  echo "发现残留临时目录" >&2
  exit 1
fi
echo "== 冒烟通过：判定序列与临时状态清理均符合预期 =="
