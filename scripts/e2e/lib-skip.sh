#!/usr/bin/env bash
# (FIX-101) Skip policy of the e2e scripts (source this; do not run it). A gated test prints a line that
# starts with `SKIPPED:` and passes; that is fine on a developer box and a defect in CI, where the suite is
# cited as evidence (docs/18 §Skip policy). `run_logged` runs a command, tees its output to a log, and with
# CI=true turns a `SKIPPED:` line into a failure naming the lines.
#
#   run_logged <logfile> <command...>     # exit status: the command's, or 1 on a SKIPPED line in CI
#
# `skipped_lines` is exposed for the self-test in scripts/guards/e2e-skip.sh.

# skipped_lines <file>: the SKIPPED: lines of a log (cargo prints them with --nocapture), minus the lines
# matching AURORA_E2E_ALLOW_SKIPPED (an extended regex). The allow-list is the explicit, commented exception
# a workflow job declares for a scenario that runs elsewhere (e.g. the chromium scenarios, which need the
# full sandbox image the PR job does not build: nightly) — never a blanket.
skipped_lines() {
  local out
  out="$(grep -iE '^[[:space:]]*SKIPPED:' "$1" || true)"
  if [ -n "$out" ] && [ -n "${AURORA_E2E_ALLOW_SKIPPED:-}" ]; then
    out="$(printf '%s\n' "$out" | grep -vE "$AURORA_E2E_ALLOW_SKIPPED" || true)"
  fi
  printf '%s' "$out"
}

run_logged() {
  local log="$1"; shift
  "$@" 2>&1 | tee "$log"
  local rc="${PIPESTATUS[0]}"
  if [ "${CI:-}" = "true" ]; then
    local skipped
    skipped="$(skipped_lines "$log")"
    if [ -n "$skipped" ]; then
      echo "FAIL: CI=true and the run skipped tests (a skipped suite is a failure in CI):" >&2
      printf '%s\n' "$skipped" >&2
      [ "$rc" -ne 0 ] || rc=1
    fi
  fi
  return "$rc"
}
