#!/usr/bin/env bash
# P5-FILES-4 (D-316, C-907, SEC-125/126): the Files feature end to end on a kind cluster — API (SFTP client), runner,
# supervisor and sandbox image together, with `terminal.enabled` on.
#
#   scripts/e2e/files.sh [--keep] [--fresh] [--skip-build] [--release-images]     (kind.sh's flags)
#
# What it runs: `scripts/e2e/kind.sh --no-upgrade-legs -- k8_files_journey`, i.e. the scenario `k8_files_journey` of
# tests/e2e-hosted/tests/k8s.rs against the chart installed with `terminal.enabled=true` (AURORA_E2E_TERMINAL=1):
# a folder of three files (mkdir with parents + PUTs), the listing, a download compared by sha256 (and the C-903
# headers), a text read and a save with If-Match, a provoked 409 `file_changed`, rename, recursive delete, a
# `.aurora` write -> 403 `path_read_only`, a symlink to /etc made in the sandbox -> 403 `path_outside_workspace`, a
# 20 MiB round trip with matching sha256, freeze -> 423 then unfreeze, a parked sandbox -> 503 `sandbox_waking` then
# success after the wake, and a canary string (uploaded inside a file) searched for in every API, runner and sandbox
# log, in `audit_log` and in `request_log`.
#
# Skip policy: with the terminal off in the cluster (AURORA_E2E_TERMINAL=0 — the shipped default) the Files routes
# answer 404 `terminal_disabled`; the scenario prints a `SKIPPED:` line with the reason and the script exits 0
# (CI=true turns that into a failure through scripts/e2e/lib-skip.sh, like every e2e script).
#
# One e2e suite at a time: it takes /tmp/aurora-e2e-gateway.lock (the lock of gateway.sh) and waits for it.
# Never run it against the `aurora-dev` cluster: use AURORA_E2E_CLUSTER (default aurora-e2e) and the kind.sh ports
# (AURORA_KIND_PORT, AURORA_E2E_IDP_PORT) of this checkout.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/e2e/lib-skip.sh
. scripts/e2e/lib-skip.sh

if [ "${AURORA_E2E_CLUSTER:-aurora-e2e}" = "aurora-dev" ]; then
  echo "files.sh: refusing to use the aurora-dev cluster" >&2
  exit 2
fi

LOCK=/tmp/aurora-e2e-gateway.lock
exec 9>"$LOCK"
if ! flock -w 3600 9; then
  echo "files.sh: gave up waiting for the e2e lock $LOCK" >&2
  exit 1
fi

export AURORA_E2E_TERMINAL="${AURORA_E2E_TERMINAL:-1}"
LOG="$ROOT/target/files-e2e.log"
mkdir -p "$ROOT/target"
# fd 9 (the lock) stays with this shell only: nothing kind.sh starts (port-forwards, containers) may inherit it.
run_logged "$LOG" bash scripts/e2e/kind.sh --no-upgrade-legs "$@" -- k8_files_journey 9>&-
rc=$?
if [ "$rc" -eq 0 ]; then
  # cargo prints the test name first (`test k8_files_journey ... SKIPPED: ...`), so the marker is mid-line.
  if grep -qE 'SKIPPED:' "$LOG"; then
    echo "files e2e: SKIPPED ($(grep -E 'SKIPPED:' "$LOG" | head -1 | sed 's/^.*SKIPPED:/SKIPPED:/'))"
  elif grep -q "test result: ok. 1 passed" "$LOG"; then
    echo "files e2e: PASS (k8_files_journey)"
  else
    echo "files e2e: FAILED: the scenario did not run (no \`test result: ok. 1 passed\` line in $LOG)" >&2
    exit 1
  fi
fi
exit "$rc"
