#!/usr/bin/env bash
# P2-E2E-1: in the kind dev profile the sandboxes must be told the IN-CLUSTER gateway proxy address.
# The default (external host + :4102) is localhost:4102 there, which is the sandbox itself: a real harness
# (claude-code) fails every model call with `API Error: 502` while the fake harness never notices.
# Renders deploy/kind/values-dev.yaml (with and without the llm-mock profile) and checks
# gateway.advertised_proxy_url is `https://aurora-gateway:4102`: the name the Kubernetes runner backend
# aliases (hostAliases, sandboxes have no DNS) to the proxy Service, and which is on the proxy certificate.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if ! command -v helm >/dev/null 2>&1; then
  echo "dev-gateway-address: helm not installed; skipping" >&2
  exit 0
fi

check() {
  local label="$1"; shift
  local out url
  out=$(helm template aurora deploy/helm/aurora -n aurora "$@")
  url=$(printf '%s\n' "$out" | grep -o 'advertised_proxy_url = "[^"]*"' | head -1 | cut -d'"' -f2)
  [ "$url" = "https://aurora-gateway:4102" ] \
    || { echo "dev-gateway-address: FAIL: $label: advertised_proxy_url is '${url:-<unset>}'" >&2; exit 1; }
}
check "values-dev" -f deploy/kind/values-dev.yaml
check "values-dev + llm-mock" -f deploy/kind/values-dev.yaml -f deploy/kind/values-llm-mock.yaml
echo "dev-gateway-address: ok"
