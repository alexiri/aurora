#!/usr/bin/env bash
# M1 acceptance: Playwright dev-IdP sign-in -> workspace -> invite -> accept (TEST-011, P1-005/RFE-004).
#   scripts/e2e/m1.sh [--kind] [playwright args]
# Default target: the host-process stack. Uses ports 4100 (web), 4101 (API), 4107 (aurora-dev-idp), a dedicated DB `aurora_e2e`,
# and tears everything down.
# --kind (phase 1, OPS-017): run against the kind dev stack instead (`just kind-up`; single URL
# http://localhost:4000, the chart's Ingress). Nothing is started or torn down; the invitation links are read
# from `kubectl logs` of the API pod. Needs signup_mode=open and the dev IdP (devIdp.enabled, allowAnyHint) as in deploy/kind/values-dev.yaml.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

KIND=0
if [ "${1:-}" = "--kind" ]; then KIND=1; shift; fi
DB=aurora_e2e
ADMIN_URL="${AURORA_E2E_ADMIN_URL:-postgres://aurora:aurora@127.0.0.1:55432/postgres}"
E2E_URL="${ADMIN_URL%/*}/$DB"
TMP="$(mktemp -d)"
API_PID=""; WEB_PID=""; IDP_PID=""; DB_CREATED=0

cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  for p in "$WEB_PID" "$API_PID" "$IDP_PID"; do
    [ -n "$p" ] && kill -TERM -- "-$p" 2>/dev/null || { [ -n "$p" ] && kill "$p" 2>/dev/null; }
  done
  sleep 1
  for p in "$WEB_PID" "$API_PID" "$IDP_PID"; do
    [ -n "$p" ] && kill -KILL -- "-$p" 2>/dev/null
  done
  if [ "$DB_CREATED" = 1 ]; then
    psql "$ADMIN_URL" -qc "DROP DATABASE IF EXISTS $DB WITH (FORCE)" >/dev/null 2>&1 || echo "WARN: could not drop $DB" >&2
  fi
  [ "$rc" -ne 0 ] && [ -f "$TMP/api.log" ] && { echo "--- API log tail ---"; tail -n 20 "$TMP/api.log" | cut -c1-400; }
  [ "$rc" -ne 0 ] && [ -f "$TMP/web.log" ] && { echo "--- web log tail ---"; tail -n 20 "$TMP/web.log"; }
  rm -rf "$TMP"
  exit "$rc"
}
trap cleanup EXIT INT TERM

if [ "$KIND" = 1 ]; then
  CTX="kind-${AURORA_DEV_CLUSTER:-aurora-dev}"
  URL="http://localhost:${AURORA_DEV_PORT:-4000}"
  curl -fsS -o /dev/null "$URL/v1/instance" || { echo "no Aurora on $URL: run 'just kind-up' first" >&2; exit 1; }
  export AURORA_E2E_API_LOG="$TMP/api.log"; : >"$AURORA_E2E_API_LOG"
  setsid kubectl --context "$CTX" -n aurora logs -f --since=1s --prefix=false --max-log-requests=5 \
    -l app.kubernetes.io/component=api >"$AURORA_E2E_API_LOG" 2>&1 &
  API_PID=$!
  sleep 2
  cd web
  AURORA_E2E_BASE_URL="$URL" pnpm exec playwright test e2e/m1.spec.ts "$@"
  exit $?
fi

for port in 4100 4101 4107; do
  if ss -ltn "sport = :$port" | grep -q LISTEN; then echo "port $port already in use" >&2; exit 1; fi
done

cargo build -p aurora-api -p aurora-testkit || { echo "cargo build failed" >&2; exit 1; }

psql "$ADMIN_URL" -qc "DROP DATABASE IF EXISTS $DB WITH (FORCE)" -c "CREATE DATABASE $DB" || exit 1
DB_CREATED=1

# Config: dev config as-is (external_url http://localhost:4100; `next dev` only serves
# HMR/client assets to localhost, not 127.0.0.1, so the browser must use localhost).
cp deploy/dev/aurora.dev.toml "$TMP/aurora.toml"

set +u
. deploy/dev/env.sh
set -u
export AURORA_CONFIG="$TMP/aurora.toml"
export AURORA_DATABASE_URL="$E2E_URL"

target/debug/aurora-api migrate >"$TMP/migrate.log" 2>&1 || { echo "migrate failed" >&2; exit 1; }

# Development IdP (P1-005, RFE-004): the API registers it as provider `dev` (deploy/dev/aurora.dev.toml).
# Any login_hint is accepted: the spec mints unique e-mails per run.
AURORA_DEV_IDP_LISTEN=127.0.0.1:4107 AURORA_DEV_IDP_USERS=deploy/dev/dev-idp-users.json AURORA_DEV_IDP_ALLOW_ANY_HINT=1 \
  setsid target/debug/aurora-dev-idp >"$TMP/idp.log" 2>&1 &
IDP_PID=$!

export AURORA_E2E_API_LOG="$TMP/api.log"
setsid target/debug/aurora-api serve >"$AURORA_E2E_API_LOG" 2>&1 &
API_PID=$!

# Dev server: production builds do not proxy /v1 (edge proxy does that), so use `next dev`.
(cd web && AURORA_API_URL=http://127.0.0.1:4101 exec setsid pnpm dev >"$TMP/web.log" 2>&1) &
WEB_PID=$!

wait_for() { # url
  for _ in $(seq 1 120); do
    curl -fsS -o /dev/null "$1" 2>/dev/null && return 0
    sleep 1
  done
  echo "timeout waiting for $1" >&2; return 1
}
wait_for http://localhost:4107/.well-known/openid-configuration || exit 1
wait_for http://127.0.0.1:4101/v1/instance || exit 1
wait_for http://localhost:4100/auth/login || exit 1

cd web
# The dev server on :4100 was started above from this checkout: reuse it (playwright.config.ts refuses
# to reuse a server it did not start unless AURORA_E2E_REUSE=1).
AURORA_E2E_REUSE=1 pnpm exec playwright test e2e/m1.spec.ts "$@"
