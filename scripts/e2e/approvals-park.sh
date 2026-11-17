#!/usr/bin/env bash
# P4-011B-5: the approval park-and-resume journey and the Web Push leg (tests/e2e-hosted/tests/
# approvals_park.rs) through the real API, gateway, runner and the fake-harness sandbox. It reuses
# scripts/e2e/hosted.sh's setup (own Postgres, sandbox network, own AURORA_E2EH_PREFIX and subnets),
# so it is a wrapper that selects the one test target; hosted.sh runs the same target in its
# default run (the PR job). Gated by AURORA_TEST_PODMAN=1. Extra arguments go to the test binary
# (a test name filter, for instance).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
export AURORA_E2EH_ONLY=approvals_park
exec bash scripts/e2e/hosted.sh "$@"
