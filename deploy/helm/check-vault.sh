#!/usr/bin/env bash
# P3-VLT-2: `helm lint` / `helm template` cases for the Vault store values and extraCaBundle.
#   - default values render no Vault, no gateway config of its own, no extra CA;
#   - kubernetes auth: per-service ServiceAccounts, projected audience-bound token, per-service roles,
#     org-id reference scopes, private-Vault egress, extra CA mount;
#   - approle auth: credentials from an existing Secret, never from values;
#   - invalid values fail the render with a message naming the key;
#   - when a built `aurora-api` exists (target/debug/aurora-api or $AURORA_API_BIN), the rendered
#     api and gateway configs pass the real configuration validation (only the unreachable
#     database may fail afterwards).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"
CHART=deploy/helm/aurora
CASES=deploy/helm/cases
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

fail() { echo "helm-vault: FAIL: $*" >&2; exit 1; }
has() { grep -Eq -- "$2" "$1" || fail "$3 (missing /$2/)"; }
lacks() { ! grep -Eq -- "$2" "$1" || fail "$3 (unexpected /$2/)"; }

# (FIX-106) An external database needs the gateway role pair or this explicit value to render at all;
# these cases are about Vault, so they take the explicit value (deploy/helm/check-gateway-role.sh covers the rest).
DB=(--set database.allowSharedLogin=true)
helm lint "$CHART" "${DB[@]}" >/dev/null || fail "lint (default values)"
helm template aurora "$CHART" -n aurora "${DB[@]}" >"$WORK/default.yaml" || fail "template (default values)"
lacks "$WORK/default.yaml" 'aurora-gateway\.toml|vault-token|extra-ca|hashicorp-vault' "default values must render no Vault or extra CA"

for c in vault-kubernetes vault-approle; do
  helm lint "$CHART" "${DB[@]}" -f "$CASES/$c.yaml" >/dev/null || fail "lint ($c)"
  helm template aurora "$CHART" -n aurora "${DB[@]}" -f "$CASES/$c.yaml" >"$WORK/$c.yaml" || fail "template ($c)"
  has "$WORK/$c.yaml" 'aurora-gateway\.toml' "$c: the gateway reads its own config"
  has "$WORK/$c.yaml" 'kind = "hashicorp-vault"' "$c: the store is rendered"
  has "$WORK/$c.yaml" 'extra_ca_bundle = "/etc/aurora-tls/' "$c: tls.extra_ca_bundle"
  has "$WORK/$c.yaml" 'mountPath: /etc/aurora-tls' "$c: extra CA mounted"
  lacks "$WORK/$c.yaml" 'token_env' "$c: token auth is development only"
done

# kubernetes auth
k="$WORK/vault-kubernetes.yaml"
has "$k" 'role = "aurora-api"' "api role"
has "$k" 'role = "aurora-gateway"' "gateway role"
has "$k" 'jwt_path = "/var/run/secrets/aurora-vault/token"' "projected token path"
has "$k" 'org_id = "0197a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b"' "reference scope keyed by org id"
has "$k" 'default_store = "vault"' "makeDefault"
has "$k" 'audience: "vault"' "projected token audience"
has "$k" 'kind: ServiceAccount' "service accounts"
has "$k" 'name: aurora-gateway$' "gateway ServiceAccount"
has "$k" 'serviceAccountName: aurora-api' "api uses its ServiceAccount"
has "$k" 'cidr: 10\.30\.0\.0/24' "egress to a private Vault"
[ "$(grep -c 'role = "aurora-api"' "$k")" = 1 ] || fail "the api role must appear exactly once (api config only)"

# approle auth
a="$WORK/vault-approle.yaml"
has "$a" 'id = "corp-vault"' "custom store id"
has "$a" 'kv_mount = "kv"' "kv mount"
has "$a" 'role_id_env = "AURORA_VAULT_ROLE_ID"' "approle env names"
has "$a" 'key: API_ROLE_ID' "api approle credentials from the Secret"
has "$a" 'key: GATEWAY_SECRET_ID' "gateway approle credentials from the Secret"
has "$a" 'secret: \{secretName: "org-ca"' "extra CA from a Secret"
lacks "$a" 'vault-token|projected' "approle needs no projected token"

# invalid values fail with a message naming the key
expect_fail() {
  local what="$1" msg="$2"; shift 2
  local out
  if out=$(helm template aurora "$CHART" -n aurora "${DB[@]}" "$@" 2>&1); then fail "$what: render should have failed"; fi
  grep -q -- "$msg" <<<"$out" || fail "$what: expected '$msg', got: $out"
}
expect_fail "no address" "vault.address is required" --set vault.enabled=true
expect_fail "token auth" "development only" --set vault.enabled=true --set vault.address=https://v:8200 --set vault.auth.method=token
expect_fail "approle without secret" "approle.existingSecret" --set vault.enabled=true --set vault.address=https://v:8200 --set vault.auth.method=approle
expect_fail "scope without org id" "orgId" -f "$CASES/vault-kubernetes.yaml" --set 'vault.referenceScopes[0].paths[0]=x/*' --set 'vault.referenceScopes[1].paths[0]=y/*'
expect_fail "both CA sources" "not both" --set extraCaBundle.existingConfigMap=a --set extraCaBundle.existingSecret=b
expect_fail "short token lifetime" "at least 600" --set vault.enabled=true --set vault.address=https://v:8200 --set vault.auth.tokenExpirationSeconds=60

# the rendered configs pass the real validation
BIN="${AURORA_API_BIN:-target/debug/aurora-api}"
if [ -x "$BIN" ] && command -v python3 >/dev/null 2>&1; then
  K="$(head -c32 /dev/zero | base64)"
  export AURORA_ENCRYPTION_KEYS="k1:$K" AURORA_ENCRYPTION_ACTIVE_KEY=k1 AURORA_SIGNING_KEY="$K" \
    AURORA_INTERNAL_SECRET="$K" AURORA_RUNNER_SESSION_KEY="$K" AURORA_DATABASE_URL=postgres://u:p@127.0.0.1:1/d \
    AURORA_VAULT_ROLE_ID=r AURORA_VAULT_SECRET_ID=s OIDC_SECRET=s
  for c in vault-kubernetes vault-approle; do
    python3 -I - "$WORK/$c.yaml" "$WORK/$c" <<'PY'
import sys, yaml
cm = [d for d in yaml.safe_load_all(open(sys.argv[1])) if d and d.get("kind") == "ConfigMap" and d["metadata"]["name"].endswith("-config")][0]
for name, body in cm["data"].items():
    open(f"{sys.argv[2]}-{name}", "w").write(body)
PY
    for f in "$WORK/$c-aurora.toml" "$WORK/$c-aurora-gateway.toml"; do
      out=$("$BIN" --config "$f" secrets migrate --to inline --dry-run 2>&1 || true)
      if grep -q 'invalid configuration' <<<"$out"; then fail "$(basename "$f"): $out"; fi
    done
  done
  echo "helm-vault: rendered configs pass aurora-api config validation"
else
  echo "helm-vault: no built aurora-api; skipping config validation" >&2
fi
echo "helm-vault: ok"
