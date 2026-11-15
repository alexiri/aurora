#!/usr/bin/env bash
# FIX-106 (D-178 Q-R5-11): `helm template` matrix for the gateway's least-privilege database role.
#   - bundled evaluation Postgres, nothing set: the chart creates the role itself (a hook Job, a
#     generated password in the generated Secret, `database.gateway_role` rendered, the gateway
#     logs in as it);
#   - external database with the role pair: the gateway uses the operator's Secret;
#   - external database without the pair: the render is REFUSED with an actionable message,
#     unless `database.allowSharedLogin=true` (the gateway then shares the main login);
#   - contradictions and the bundled opt-outs behave as documented.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
CHART=deploy/helm/aurora
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail() { echo "helm-gateway-role: FAIL: $*" >&2; exit 1; }
has() { grep -Eq -- "$2" "$1" || fail "$3 (missing /$2/)"; }
lacks() { ! grep -Eq -- "$2" "$1" || fail "$3 (unexpected /$2/)"; }

render() { # name, helm args...
  local name="$1"; shift
  helm lint "$CHART" "$@" >/dev/null || fail "lint ($name)"
  helm template aurora "$CHART" -n aurora "$@" >"$WORK/$name.yaml" || fail "template ($name)"
}

refused() { # name, expected message fragment, helm args...
  local name="$1" msg="$2"; shift 2
  local out
  if out=$(helm template aurora "$CHART" -n aurora "$@" 2>&1); then fail "$name: the render should have been refused"; fi
  grep -q -- "$msg" <<<"$out" || fail "$name: expected '$msg', got: $out"
}

# "<secret name>/<key>" the gateway Deployment reads AURORA_DATABASE_URL from.
gateway_url_ref() {
  python3 -I - "$1" <<'PY'
import sys, yaml
for d in yaml.safe_load_all(open(sys.argv[1])):
    if d and d.get("kind") == "Deployment" and d["metadata"]["name"].endswith("-gateway"):
        for c in d["spec"]["template"]["spec"]["containers"]:
            for e in c.get("env", []):
                if e["name"] == "AURORA_DATABASE_URL":
                    r = e["valueFrom"]["secretKeyRef"]
                    print(f'{r["name"]}/{r["key"]}')
                    sys.exit(0)
sys.exit("gateway AURORA_DATABASE_URL not found")
PY
}

# 1. bundled evaluation Postgres, nothing else: automatic role.
render bundled --set postgres.enabled=true
has "$WORK/bundled.yaml" 'name: aurora-gateway-role$' "bundled: the gateway-role Job"
has "$WORK/bundled.yaml" 'CREATE ROLE %I LOGIN PASSWORD' "bundled: the Job creates the role"
has "$WORK/bundled.yaml" 'gateway_role = "aurora_gateway"' "bundled: migrate grants the role"
has "$WORK/bundled.yaml" 'name: GW_ROLE_AUTO, value: "true"' "bundled: the secrets hook generates the role's password"
[ "$(gateway_url_ref "$WORK/bundled.yaml")" = "aurora-secrets/AURORA_GATEWAY_DATABASE_URL" ] \
  || fail "bundled: the gateway must log in as the generated role, got $(gateway_url_ref "$WORK/bundled.yaml")"

# 2. external database with the role pair.
render pair --set database.gatewayRole=aurora_gateway --set database.gatewayExistingSecret=gw-db
lacks "$WORK/pair.yaml" 'aurora-gateway-role$' "pair: no automatic role Job"
has "$WORK/pair.yaml" 'gateway_role = "aurora_gateway"' "pair: migrate grants the operator's role"
[ "$(gateway_url_ref "$WORK/pair.yaml")" = "gw-db/AURORA_DATABASE_URL" ] || fail "pair: the gateway must use the operator's Secret"

# 3. external database without the pair: refused, with the way out named.
refused "external without the role" "least-privilege database role"
refused "external without the role (names the value)" "database.allowSharedLogin"
refused "only the role name" "go together" --set database.gatewayRole=aurora_gateway
refused "only the Secret" "go together" --set database.gatewayExistingSecret=gw-db

# 4. external database, explicit shared login.
render shared --set database.allowSharedLogin=true
lacks "$WORK/shared.yaml" 'aurora-gateway-role$' "shared: no role Job"
lacks "$WORK/shared.yaml" 'gateway_role' "shared: no gateway role configured"
[ "$(gateway_url_ref "$WORK/shared.yaml")" = "aurora-database/AURORA_DATABASE_URL" ] \
  || fail "shared: the gateway uses the main login, got $(gateway_url_ref "$WORK/shared.yaml")"

# 5. contradictions and the bundled opt-outs.
refused "pair and shared login" "contradict" --set database.allowSharedLogin=true \
  --set database.gatewayRole=aurora_gateway --set database.gatewayExistingSecret=gw-db
render bundled-shared --set postgres.enabled=true --set database.allowSharedLogin=true
lacks "$WORK/bundled-shared.yaml" 'aurora-gateway-role$' "bundled + shared login: no automatic role"
[ "$(gateway_url_ref "$WORK/bundled-shared.yaml")" = "aurora-secrets/AURORA_DATABASE_URL" ] || fail "bundled + shared login: the gateway uses the main login"
render bundled-pair --set postgres.enabled=true --set database.gatewayRole=aurora_gateway --set database.gatewayExistingSecret=gw-db
lacks "$WORK/bundled-pair.yaml" 'aurora-gateway-role$' "bundled + pair: the operator's role wins"
[ "$(gateway_url_ref "$WORK/bundled-pair.yaml")" = "gw-db/AURORA_DATABASE_URL" ] || fail "bundled + pair: the gateway uses the pair"

echo "helm-gateway-role: ok"
