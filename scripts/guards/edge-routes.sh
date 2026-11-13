#!/usr/bin/env bash
# Edge configuration covers all route prefixes (docs/18 "Architecture guards", docs/15 "Networking").
# Verifies the Helm Ingress and the Helm HTTPRoute (the compose Caddyfile is gone: phase 1, OPS-017), each
# rendered with the chart defaults and with the kind dev profile (deploy/kind/values-dev.yaml):
#   /v1/* -> api:4101, /connectors/* -> connector-host:4110, everything else -> web:4100,
#   /internal and /metrics never routed to api.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "edge-routes: FAIL: $*" >&2; exit 1; }

# --- Helm Ingress / HTTPRoute ---
# shellcheck source=scripts/guards/lib.sh
source scripts/guards/lib.sh
require_helm edge-routes || exit 0
check_render() {
  local label="$1"; shift
  helm template guard deploy/helm/aurora "$@" | python3 -I -c '
import sys, yaml
label = sys.argv[1]
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
routes = []  # (host, path, backend service suffix, port)
for d in docs:
    if d["kind"] == "Ingress" and "connector" not in d["metadata"]["name"]:
        for r in d["spec"]["rules"]:
            for p in r["http"]["paths"]:
                b = p["backend"]["service"]
                routes.append((r.get("host", ""), p["path"], b["name"], b["port"]["number"]))
    elif d["kind"] == "HTTPRoute":
        for r in d["spec"]["rules"]:
            # Only hostname-scoped rules belong to that hostname; shared rules apply to
            # every spec-level hostname and are checked by their own exact-host branch.
            for m in r["matches"]:
                for b in r["backendRefs"]:
                    if r.get("hostnames"):
                        for h in r["hostnames"]:
                            routes.append((h, m["path"]["value"], b["name"], b["port"]))
                    else:
                        # Shared rule: attach to each non-wildcard spec hostname only.
                        for h in d["spec"].get("hostnames", []):
                            if not h.startswith("*."):
                                routes.append((h, m["path"]["value"], b["name"], b["port"]))
def fail(msg):
    sys.exit("edge-routes: FAIL: %s: %s (routes=%s)" % (label, msg, routes))
if not routes:
    fail("no routes rendered")
for host, path, svc, port in routes:
    if path.startswith(("/internal", "/metrics")):
        fail("routes " + path)
    if host.startswith("*."):
        # (P4-PRV-2) the previews wildcard serves everything through the relay, nothing else.
        if not (svc.endswith("-relay") and port == 4112):
            fail("wildcard host %s must route to relay:4112 only" % host)
        continue
    if path == "/v1" and not (svc.endswith("-api") and port == 4101):
        fail("/v1 must go to api:4101")
    # (P4-GRP-2) SCIM: /scim (the /scim/v2 plane) is served by the API, only when enabled.
    if path == "/scim" and not (svc.endswith("-api") and port == 4101):
        fail("/scim must go to api:4101")
    if path == "/" and not (svc.endswith("-web") and port == 4100):
        fail("/ must go to web:4100")
    if path == "/connectors" and not (svc.endswith("-connector-host") and port == 4110):
        fail("/connectors must go to connector-host:4110")
if not any(p == "/v1" for _, p, _, _ in routes): fail("no /v1 route")
if not any(p == "/" and not h.startswith("*.") for h, p, _, _ in routes): fail("no / route")
# (P4-MCON) the platform callbacks of managed connectors are served by the connector host on one port.
if not any(p == "/connectors" for _, p, _, _ in routes): fail("no /connectors route (connector host)")
print("edge-routes: %s ok" % label)
' "$label"
}
check_render "helm Ingress"
check_render "helm HTTPRoute" --set ingress.enabled=false --set gatewayApi.enabled=true
check_render "kind dev profile Ingress" -f deploy/kind/values-dev.yaml
# (P4-GRP-2) With the SCIM switch on, the edge must route /scim to the API in both edge types.
check_render "helm Ingress with scim" --set scim.enabled=true
check_render "helm HTTPRoute with scim" --set ingress.enabled=false --set gatewayApi.enabled=true --set scim.enabled=true
# FIX-068: capture the render first. `helm … | grep -q` under pipefail passed exactly when the
# forbidden resource DID render (grep -q exits on the first match, helm takes SIGPIPE, the
# pipeline is non-zero, the `if` is false). A failed render must fail the guard too.
default_render="$(helm template guard deploy/helm/aurora)" || fail "the default chart render failed"
[ -n "$default_render" ] || fail "the default chart render is empty"
if grep -q "path: /scim" <<<"$default_render"; then
  fail "/scim route rendered while scim.enabled is false (SEC-72: the surface must 404)"
fi

# --- (P4-PRV-2) port previews: wildcard host only when enabled, only to the relay ---
# Defaults (previews off): no relay resource of any kind may render.
if grep -qE 'component: relay|aurora-relay|-relay,' <<<"$default_render"; then
  fail "relay resources rendered while relay.enabled is false (previews off must change nothing)"
fi
# Enabled: the wildcard host `*.<domain>` must be the relay and only the relay; the dashboard
# host must still route / to web. A preview domain on the relay with previews disabled (or the
# reverse) must fail to render at all (the chart's own fail guard), so just render both edges.
# (FIX-056) Single-switch form: relay.enabled mirrors config.previews.enabled, so the guard
# renders with the top-level switch only.
check_render "helm Ingress with previews" \
  --set relay.enabled=true --set relay.domain=previews.guard.invalid
check_render "helm HTTPRoute with previews" \
  --set ingress.enabled=false --set gatewayApi.enabled=true \
  --set relay.enabled=true --set relay.domain=previews.guard.invalid
helm template guard deploy/helm/aurora \
  --set relay.enabled=true --set relay.domain=previews.guard.invalid \
  | python3 -I -c '
import sys, yaml
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
wild = []     # ingress rules with a wildcard host
dash_web = False
relay_svcs = 0
for d in docs:
    if d["kind"] == "Ingress" and "connector" not in d["metadata"]["name"]:
        for r in d["spec"]["rules"]:
            host = r.get("host", "")
            if host.startswith("*."):
                wild.append((host, r["http"]["paths"]))
            else:
                for p in r["http"]["paths"]:
                    if p["path"] == "/" and p["backend"]["service"]["name"].endswith("-web"):
                        dash_web = True
    elif d["kind"] == "HTTPRoute":
        for host in d["spec"].get("hostnames", []):
            if host.startswith("*."):
                for r in d["spec"]["rules"]:
                    if host in r.get("hostnames", []):
                        for b in r["backendRefs"]:
                            wild.append((host, [{"backend": {"service": {"name": b["name"], "port": {"number": b["port"]}}}}]))
if not wild:
    sys.exit("edge-routes: FAIL: previews on but no wildcard host rendered")
for host, paths in wild:
    if not all(p["backend"]["service"]["name"].endswith("-relay") and p["backend"]["service"]["port"]["number"] == 4112 for p in paths):
        sys.exit("edge-routes: FAIL: wildcard host %s routes something other than relay:4112" % host)
if not dash_web:
    sys.exit("edge-routes: FAIL: previews on but the dashboard / route to web is gone")
print("edge-routes: previews wildcard ok (relay only, dashboard untouched)")
'
# Enabled without a domain must fail to render (the chart guards partial setups).
if helm template guard deploy/helm/aurora --set relay.enabled=true >/dev/null 2>&1; then
  fail "relay.enabled without relay.domain rendered (a preview domain is required)"
fi
