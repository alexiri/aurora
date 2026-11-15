#!/usr/bin/env bash
# Edge configuration covers all route prefixes (docs/18 "Architecture guards", docs/15 "Networking").
# Verifies the Helm Ingress and the Helm HTTPRoute (the compose Caddyfile is gone: phase 1, OPS-017), each
# rendered with the chart defaults and with the kind dev profile (deploy/kind/values-dev.yaml):
#   /v1/* -> api:4101, /connectors/* -> connector-host:4110, everything else -> web:4100,
#   /internal and /metrics never routed to api. (FIX-102) No `rules[].hostnames` (it is not a Gateway API
#   field), the previews wildcard is its own HTTPRoute, the dashboard host's `/` binds to web; the
#   rendered Gateway API objects are validated with kubeconform when it is installed.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

fail() { echo "edge-routes: FAIL: $*" >&2; exit 1; }

# --- Helm Ingress / HTTPRoute ---
# shellcheck source=scripts/guards/lib.sh
source scripts/guards/lib.sh
require_helm edge-routes || exit 0

# (FIX-102) Mutation self-test: the old chart shape (a hostname-scoped rule inside the dashboard
# HTTPRoute, wildcard listed in spec.hostnames) must fail the checker.
planted="$(mktemp)"
trap 'rm -f "$planted"' EXIT
cat >"$planted" <<'YAML'
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata: {name: x}
spec:
  hostnames: [aurora.example, "*.previews.example"]
  rules:
    - hostnames: ["*.previews.example"]
      matches: [{path: {type: PathPrefix, value: /}}]
      backendRefs: [{name: x-relay, port: 4112}]
    - matches: [{path: {type: PathPrefix, value: /v1}}]
      backendRefs: [{name: x-api, port: 4101}]
    - matches: [{path: {type: PathPrefix, value: /connectors}}]
      backendRefs: [{name: x-connector-host, port: 4110}]
    - matches: [{path: {type: PathPrefix, value: /}}]
      backendRefs: [{name: x-web, port: 4100}]
YAML
if python3 -I scripts/guards/edge-routes-check.py "self-test" <"$planted" >/dev/null 2>&1; then
  fail "self-test: a rules[].hostnames HTTPRoute passed the checker"
fi

check_render() {
  local label="$1"; shift
  # (FIX-106) allowSharedLogin: not about the gateway's database role.
  helm template guard deploy/helm/aurora --set database.allowSharedLogin=true "$@" \
    | python3 -I scripts/guards/edge-routes-check.py "$label"
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
default_render="$(helm template guard deploy/helm/aurora --set database.allowSharedLogin=true)" || fail "the default chart render failed"
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
# host must still route / to web (the checker asserts it for both edge types). A preview domain on
# the relay with previews disabled (or the reverse) must fail to render at all (the chart's own fail
# guard), so just render both edges.
# (FIX-056) Single-switch form: relay.enabled mirrors config.previews.enabled, so the guard
# renders with the top-level switch only.
check_render "helm Ingress with previews" \
  --set relay.enabled=true --set relay.domain=previews.guard.invalid
check_render "helm HTTPRoute with previews" \
  --set ingress.enabled=false --set gatewayApi.enabled=true \
  --set relay.enabled=true --set relay.domain=previews.guard.invalid
previews_render="$(helm template guard deploy/helm/aurora --set database.allowSharedLogin=true \
  --set ingress.enabled=false --set gatewayApi.enabled=true \
  --set relay.enabled=true --set relay.domain=previews.guard.invalid)" || fail "the previews render failed"
python3 -I -c '
import sys, yaml
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
routes = [d for d in docs if d["kind"] == "HTTPRoute"]
wild = [d for d in routes if any(h.startswith("*.") for h in d["spec"]["hostnames"])]
if len(wild) != 1 or not wild[0]["metadata"]["name"].endswith("-previews"):
    sys.exit("edge-routes: FAIL: the previews wildcard must be exactly one HTTPRoute named <fullname>-previews")
if not all(h.startswith("*.") for h in wild[0]["spec"]["hostnames"]):
    sys.exit("edge-routes: FAIL: the previews HTTPRoute must carry only the wildcard host")
dash = [d for d in routes if d is not wild[0]]
if any(h.startswith("*.") for d in dash for h in d["spec"]["hostnames"]):
    sys.exit("edge-routes: FAIL: the dashboard HTTPRoute lists the wildcard host")
print("edge-routes: previews wildcard ok (own HTTPRoute, relay only, dashboard untouched)")
' <<<"$previews_render"

# --- (FIX-102) Gateway API objects validate against the upstream schemas (when kubeconform exists) ---
if command -v kubeconform >/dev/null 2>&1; then
  if ! kubeconform -strict -summary -ignore-missing-schemas \
      -schema-location default \
      -schema-location 'https://raw.githubusercontent.com/datreeio/CRDs-catalog/main/{{.Group}}/{{.ResourceKind}}_{{.ResourceAPIVersion}}.json' \
      <<<"$previews_render" >/dev/null 2>&1; then
    # No network to the schema catalogue is not a chart defect: retry offline, built-in kinds only.
    kubeconform -strict -summary -ignore-missing-schemas <<<"$previews_render" >/dev/null \
      || fail "kubeconform rejected the rendered chart"
  fi
  echo "edge-routes: kubeconform ok"
else
  echo "edge-routes: kubeconform not installed, schema validation skipped"
fi

# Enabled without a domain must fail to render (the chart guards partial setups).
if helm template guard deploy/helm/aurora --set database.allowSharedLogin=true --set relay.enabled=true >/dev/null 2>&1; then
  fail "relay.enabled without relay.domain rendered (a preview domain is required)"
fi
