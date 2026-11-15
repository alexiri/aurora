#!/usr/bin/env bash
# FIX-101 (docs/18 §Skip policy): the e2e scripts fail on a SKIPPED: line when CI=true, and a gated Rust
# suite that skips is a failure there (aurora_testkit::require_or_skip).
#   1. scripts/e2e/lib-skip.sh behaves: CI=true + a SKIPPED: line -> non-zero; CI unset -> zero; the declared
#      allow-list (AURORA_E2E_ALLOW_SKIPPED) exempts exactly the lines it matches.
#   2. every e2e script that runs a cargo test of an e2e crate goes through run_logged.
#   3. the env-gated suites named in docs/18 call require_or_skip (their gate is a workflow variable).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "e2e-skip: FAIL: $*" >&2; exit 1; }

# shellcheck source=scripts/e2e/lib-skip.sh
source scripts/e2e/lib-skip.sh

# 1. behaviour (self-test with planted output)
log="$(mktemp)"
trap 'rm -f "$log"' EXIT
planted() { printf 'test a ... ok\nSKIPPED: AURORA_E2E_FRONTDOOR is not set\ntest b ... ok\n'; }
if (CI=true run_logged "$log" planted >/dev/null 2>&1); then
  fail "self-test: CI=true with a SKIPPED: line returned success"
fi
(CI="" run_logged "$log" planted >/dev/null 2>&1) || fail "self-test: a SKIPPED: line fails outside CI (a developer box may skip)"
(CI=true AURORA_E2E_ALLOW_SKIPPED="AURORA_E2E_FRONTDOOR is not set" run_logged "$log" planted >/dev/null 2>&1) \
  || fail "self-test: the declared allow-list did not exempt its line"
(CI=true AURORA_E2E_ALLOW_SKIPPED="something else" run_logged "$log" planted >/dev/null 2>&1) \
  && fail "self-test: an allow-list that does not match exempted a SKIPPED: line"

# 2. the scripts go through run_logged
for s in gateway hosted channels; do
  grep -q 'run_logged' "scripts/e2e/$s.sh" || fail "scripts/e2e/$s.sh does not run its cargo test through run_logged (a SKIPPED: line would pass in CI)"
done

# 3. the gated suites use the helper
for f in crates/aurora-counters/tests/it/conformance.rs services/aurora-runner/tests/it/podman.rs; do
  grep -q 'require_or_skip' "$f" || fail "$f: an env-gated suite must call aurora_testkit::require_or_skip"
done

echo "e2e-skip: ok"
