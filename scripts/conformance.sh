#!/usr/bin/env bash
# Connector conformance suite (TEST-015, docs/13 §7).
#
#   scripts/conformance.sh                       # in-process fake connector (same as `cargo test`)
#   scripts/conformance.sh target/debug/aurora-connector-fake   # binary mode
#
# Binary mode spawns the connector once per scenario against an in-memory fake
# core on a loopback port. The binary is configured through its environment:
#   AURORA_CORE_URL, AURORA_CONNECTOR_TOKEN   set by the suite
#   AURORA_FAKE_CONTROL=true, AURORA_FAKE_PORT   the fake connector's /fake/* control API
#   AURORA_FAKE_CAPABILITIES                  capability set for the fallback scenario
# The suite's platform driver is the fake connector's (`/fake/*`). Another
# connector's conformance run needs its own `PlatformDriver` (see
# tests/conformance/connector/src/driver.rs) and a test target wiring it up.
# Extra arguments are passed to `cargo test` (after `--`).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
if [ $# -ge 1 ] && [[ "$1" != -* ]]; then
  BIN="$1"; shift
  case "$BIN" in /*) ;; *) BIN="$ROOT/$BIN" ;; esac
  if [ ! -x "$BIN" ]; then echo "conformance: $BIN is not an executable (cargo build -p aurora-connector-fake?)" >&2; exit 2; fi
  export CONFORMANCE_CONNECTOR_BIN="$BIN"
  echo "conformance: binary mode: $BIN"
else
  unset CONFORMANCE_CONNECTOR_BIN
  echo "conformance: in-process mode (aurora-connector-fake)"
fi
exec cargo test -p aurora-conformance-connector -- "$@"
