#!/usr/bin/env bash
set -euo pipefail

export PATH="${HOME}/.cargo/bin:${PATH}"
cd "$(dirname "$0")/.."

sha256sum -c artifacts/evidence/original.sha256
cargo test
cargo run --release -- analyze \
  --video artifacts/original/screen.mkv \
  --events artifacts/original/mouse-events.jsonl \
  --out artifacts/out
cargo run --release -- verify --out artifacts/out | tee artifacts/evidence/acceptance.txt
sha256sum -c artifacts/evidence/original.sha256
