#!/usr/bin/env bash
# Connector conformance suite (TEST-015, docs/13 §7).
#
#   scripts/conformance.sh                       # in-process fake connector (same as `cargo test`)
#   scripts/conformance.sh target/debug/aurora-connector-fake   # binary mode
#   scripts/conformance.sh --mattermost          # the Mattermost connector against a Mattermost container
#
# Binary mode spawns the connector once per scenario against an in-memory fake
# core on a loopback port. The binary is configured through its environment:
#   AURORA_CORE_URL, AURORA_CONNECTOR_TOKEN   set by the suite
#   AURORA_FAKE_CONTROL=true, AURORA_FAKE_PORT   the fake connector's /fake/* control API
#   AURORA_FAKE_CAPABILITIES                  capability set for the fallback scenario
# The suite's platform driver is the fake connector's (`/fake/*`). Another
# connector's conformance run needs its own `PlatformDriver` (see
# tests/conformance/connector/src/driver.rs) and a test target wiring it up.
#
# --mattermost builds aurora-connector-mattermost, starts (or reuses) the pinned
# Mattermost container (scripts/mattermost/setup.sh), and runs the scenarios with
# the Mattermost driver (tests/conformance/connector/src/mattermost_driver.rs),
# one at a time. The container is removed afterwards unless AURORA_MM_KEEP=1.
# Scenario steps the platform cannot take part in are skipped through
# `PlatformDriver::quirks()`; the exclusions are listed in connectors/mattermost/README.md:
#   - s02/s05  other users' email_verified flag (Mattermost 12 hides it over REST),
#              and "a person with no email" (every Mattermost account has one)
#   - s05      the POST /link-start step (no person-initiated action in shared mode)
#   - s08(a)   presence.removed for a dedicated presence (dedicated mode is MM-002)
# Extra arguments are passed to `cargo test` (after `--`).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [ "${1:-}" = "--mattermost" ]; then
  shift
  cargo build -p aurora-connector-mattermost || exit 2
  bash "$ROOT/scripts/mattermost/setup.sh" up || exit 2
  # shellcheck disable=SC1091
  set -a; . "$ROOT/target/mattermost/env"; set +a
  export CONFORMANCE_PLATFORM=mattermost
  export CONFORMANCE_CONNECTOR_BIN="$ROOT/target/debug/aurora-connector-mattermost"
  echo "conformance: mattermost mode: $CONFORMANCE_CONNECTOR_BIN against $AURORA_MM_URL"
  cargo test -p aurora-conformance-connector --test scenarios -- --test-threads=1 "$@"
  rc=$?
  if [ "${AURORA_MM_KEEP:-0}" != 1 ]; then bash "$ROOT/scripts/mattermost/setup.sh" down; fi
  exit $rc
fi

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
