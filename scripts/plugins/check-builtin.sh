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

# Besides plugins/builtin/*, the test-only built-in-shaped packages that prove framework features
# are checked too (plugins/fixtures/hook-example: hook-capable built-ins, P2-CAT-H). They are never
# shipped: scripts/plugins/build-builtin.sh only packs plugins/builtin/*.
EXTRA=(plugins/fixtures/hook-example)

dirs=()
if [ "$#" -gt 0 ]; then
  for p in "$@"; do
    if [ -d "plugins/builtin/$p" ]; then dirs+=("plugins/builtin/$p")
    elif [ -d "plugins/fixtures/$p" ]; then dirs+=("plugins/fixtures/$p")
    else echo "no such built-in package: $p" >&2; exit 2; fi
  done
else
  for d in plugins/builtin/*/; do dirs+=("${d%/}"); done
  dirs+=("${EXTRA[@]}")
fi
# Host-target unit tests of hook guest crates (e.g. the github-app JWT claims and request headers,
# which recorded HTTP cannot assert): `cargo test --locked` in every plugins/builtin/*/hooks crate
# that has tests. A failure fails the check.
for d in "${dirs[@]}"; do
  case "$d" in plugins/builtin/*) ;; *) continue ;; esac
  h="$d/hooks"
  [ -f "$h/Cargo.toml" ] || continue
  grep -rqE '#\[(cfg\(test\)|test)\]' "$h/src" "$h/tests" 2>/dev/null || continue
  echo "cargo test --locked ($h)"
  (cd "$h" && cargo test --locked -q) || { echo "FAIL hook unit tests: $h" >&2; exit 1; }
done

exec python3 -I scripts/plugins/check_builtin.py "$AURORA" "${dirs[@]}"
