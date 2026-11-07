#!/usr/bin/env bash
# Edge configuration covers all route prefixes (docs/18 "Architecture guards", docs/15 "Networking").
# Verifies the Helm Ingress and the Helm HTTPRoute (the compose Caddyfile is gone: phase 1, OPS-017), each
# rendered with the chart defaults and with the kind dev profile (deploy/kind/values-dev.yaml):
#   /v1/* -> api:4101, everything else -> web:4100, /internal and /metrics never routed to api.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "edge-routes: FAIL: $*" >&2; exit 1; }

# --- Helm Ingress / HTTPRoute ---
if ! command -v helm >/dev/null 2>&1; then
  echo "edge-routes: helm not installed; skipping chart checks" >&2
  exit 0
fi
check_render() {
  local label="$1"; shift
  helm template guard deploy/helm/aurora "$@" | python3 -I -c '
import sys, yaml
label = sys.argv[1]
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
routes = []  # (path, prefix?, backend service suffix, port)
for d in docs:
    if d["kind"] == "Ingress" and "connector" not in d["metadata"]["name"]:
        for r in d["spec"]["rules"]:
            for p in r["http"]["paths"]:
                b = p["backend"]["service"]
                routes.append((p["path"], b["name"], b["port"]["number"]))
    elif d["kind"] == "HTTPRoute":
        for r in d["spec"]["rules"]:
            for m in r["matches"]:
                for b in r["backendRefs"]:
                    routes.append((m["path"]["value"], b["name"], b["port"]))
def fail(msg):
    sys.exit("edge-routes: FAIL: %s: %s (routes=%s)" % (label, msg, routes))
if not routes:
    fail("no routes rendered")
for path, svc, port in routes:
    if path.startswith(("/internal", "/metrics")):
        fail("routes " + path)
    if path == "/v1" and not (svc.endswith("-api") and port == 4101):
        fail("/v1 must go to api:4101")
    if path == "/" and not (svc.endswith("-web") and port == 4100):
        fail("/ must go to web:4100")
if not any(p == "/v1" for p, _, _ in routes): fail("no /v1 route")
if not any(p == "/" for p, _, _ in routes): fail("no / route")
print("edge-routes: %s ok" % label)
' "$label"
}
check_render "helm Ingress"
check_render "helm HTTPRoute" --set ingress.enabled=false --set gatewayApi.enabled=true
check_render "kind dev profile Ingress" -f deploy/kind/values-dev.yaml
