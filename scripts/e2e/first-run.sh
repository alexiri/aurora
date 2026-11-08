#!/usr/bin/env bash
# P2-E2E-1 (decision D-78): the phase-2 exit journey, through the dashboard only, on the kind dev stack with the
# mock LLM: a new user signs in (dev IdP), adds a LiteLLM key, grants it to the default agent, says hello and
# gets the VidaiMock answer ("pong") back through the gateway, from the REAL claude-code harness in a sandbox Pod.
#
#   scripts/e2e/first-run.sh [--up] [playwright args]
#
# Without --up the stack must already be running:  just kind-up --llm-mock   (docs/operator/install-kind-dev.md).
# With --up it is brought up (or refreshed; warm = minutes, cold = ~25 min) first: that is the only scripted
# part; the journey itself (web/e2e/first-run.spec.ts) uses no API seeding and no database shortcuts.
# Run it again on the same stack: every run signs in a fresh user.
# Env: AURORA_DEV_CLUSTER (aurora-dev) and AURORA_DEV_PORT (4000), as for deploy/dev/kind-dev.sh.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

CLUSTER="${AURORA_DEV_CLUSTER:-aurora-dev}"
PORT="${AURORA_DEV_PORT:-4000}"
CTX="kind-$CLUSTER"
URL="http://localhost:$PORT"
k() { kubectl --context "$CTX" "$@"; }

if [ "${1:-}" = "--up" ]; then
  shift
  bash deploy/dev/kind-dev.sh up --llm-mock || { echo "kind-up --llm-mock failed" >&2; exit 1; }
fi

# Pods report Running before the ingress routes to them (nightly CI hit a 503 right after kind-up),
# so wait up to FIRST_RUN_READY_SECS (default 300) for the API to answer through the ingress.
ready_deadline=$(( $(date +%s) + ${FIRST_RUN_READY_SECS:-300} ))
until curl -fsS -o /dev/null "$URL/v1/instance"; do
  if [ "$(date +%s)" -ge "$ready_deadline" ]; then
    echo "no Aurora on $URL: run 'just kind-up --llm-mock' first (or pass --up)" >&2
    exit 1
  fi
  sleep 5
done
# The journey needs the --llm-mock profile: the mock, and the sandbox image that holds the claude-code CLI.
k -n aurora-dev-mock get deploy litellm >/dev/null 2>&1 || { echo "the llm-mock profile is not installed in $CTX: 'just kind-up --llm-mock'" >&2; exit 1; }
k -n aurora get cm -o yaml 2>/dev/null | grep -q "aurora-sandbox-llm" || { echo "runner.image is not the claude-code sandbox image: re-run 'just kind-up --llm-mock'" >&2; exit 1; }

collect() {
  echo "--- pods ---"; k -n aurora get pods 2>&1 | tail -n 12; k -n aurora-sandboxes get pods 2>&1 | tail -n 5
  for d in api gateway runner; do echo "--- $d log tail ---"; k -n aurora logs "deploy/aurora-$d" --tail=15 2>&1 | cut -c1-400; done
  echo "--- litellm log tail ---"; k -n aurora-dev-mock logs deploy/litellm --tail=10 2>&1 | cut -c1-300
}

T0=$SECONDS
cd web
AURORA_E2E_BASE_URL="$URL" pnpm exec playwright test e2e/first-run.spec.ts "$@"
rc=$?
echo "first-run journey: exit $rc after $((SECONDS - T0))s"
[ "$rc" -ne 0 ] && collect
exit "$rc"
