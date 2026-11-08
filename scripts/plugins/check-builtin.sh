#!/usr/bin/env bash
# Checks for every built-in package (or the named ones): validate, test, fixture coverage, scopes,
# token, docs (plugins/builtin/README.md). Offline. Needs the aurora and aurora-gateway binaries
# (built here unless SKIP_CARGO=1; CI's rust-build job has already built them).
#
#   bash scripts/plugins/check-builtin.sh [package-id...]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if [ -z "${SKIP_CARGO:-}" ]; then
  cargo build -q -p aurora-cli -p aurora-gateway
fi
export AURORA_GATEWAY_BIN="${AURORA_GATEWAY_BIN:-$ROOT/target/debug/aurora-gateway}"
AURORA="${AURORA_BIN:-$ROOT/target/debug/aurora}"

dirs=()
if [ "$#" -gt 0 ]; then
  for p in "$@"; do dirs+=("plugins/builtin/$p"); done
else
  for d in plugins/builtin/*/; do dirs+=("${d%/}"); done
fi
exec python3 -I scripts/plugins/check_builtin.py "$AURORA" "${dirs[@]}"
