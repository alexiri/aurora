#!/usr/bin/env bash
# FIX-010 (D-87, supersedes the P2-E2E-1 workaround): the kind dev profile must NOT override the gateway
# addresses. Hosted sandboxes get `https://aurora-gateway:4102` from the Kubernetes backend default of
# gateway.sandbox_proxy_url; BYO agents keep the external gateway.advertised_proxy_url. Overriding
# advertised_proxy_url to the in-cluster name (the old workaround) hands BYO agents an address they
# cannot resolve. Renders deploy/kind/values-dev.yaml (with and without the llm-mock profile) and checks:
#   - config has no advertised_proxy_url and no sandbox_proxy_url;
#   - runner.backend is kubernetes (the default that selects the sandbox address);
#   - the proxy certificate names include `aurora-gateway` (the name sandboxes verify).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

if ! command -v helm >/dev/null 2>&1; then
  echo "dev-gateway-address: helm not installed; skipping" >&2
  exit 0
fi

check() {
  local label="$1"; shift
  local out
  out=$(helm template aurora deploy/helm/aurora -n aurora "$@")
  if grep -Eq '^[[:space:]]*(advertised_proxy_url|sandbox_proxy_url)[[:space:]]*=' <<<"$out"; then
    echo "dev-gateway-address: FAIL: $label: the dev profile overrides a gateway proxy address" >&2
    exit 1
  fi
  grep -Eq '^[[:space:]]*backend[[:space:]]*=[[:space:]]*"kubernetes"' <<<"$out" \
    || { echo "dev-gateway-address: FAIL: $label: runner.backend is not kubernetes" >&2; exit 1; }
  grep -Eq '"aurora-gateway"' <<<"$out" \
    || { echo "dev-gateway-address: FAIL: $label: proxy certificate names lack aurora-gateway" >&2; exit 1; }
}
check "values-dev" -f deploy/kind/values-dev.yaml
check "values-dev + llm-mock" -f deploy/kind/values-dev.yaml -f deploy/kind/values-llm-mock.yaml
echo "dev-gateway-address: ok"
