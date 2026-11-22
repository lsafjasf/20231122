#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo test --all-targets
cargo run --bin bframe-evidence -- evidence
cargo run --bin bframe-evidence -- --replay evidence
