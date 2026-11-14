#!/usr/bin/env bash
# Runs every architecture guard and the schema drift check. Fails on the first failure.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

echo "== schema guard (docs/03 vs migrations)"
python3 scripts/extract-schema.py --check

echo "== no-integration-names (SEC-33)"
bash scripts/guards/no-integration-names.sh

echo "== dep-graph"
bash scripts/guards/dep-graph.sh

echo "== edge-routes (Helm Ingress/HTTPRoute)"
bash scripts/guards/edge-routes.sh

echo "== hook-policies (OPS-024: pre-upgrade hooks admitted by hook-rendered policies)"
bash scripts/guards/hook-policies.sh

echo "== helm-gateway-role (FIX-106: the gateway's least-privilege role is automatic, required, or explicitly waived)"
source scripts/guards/lib.sh
if require_helm helm-gateway-role; then bash deploy/helm/check-gateway-role.sh; fi

echo "== limits-readers (FIX-116: every limits.rs constant has a reader and a docs/02 row)"
bash scripts/guards/limits-readers.sh

echo "== no-dev-idp-in-release (SEC-42)"
bash scripts/guards/no-dev-idp-in-release.sh

echo "== no-live-tokens (built-in plugin fixtures)"
bash scripts/guards/no-live-tokens.sh

echo "== dev-gateway-address (kind dev profile: in-cluster gateway proxy)"
bash scripts/guards/dev-gateway-address.sh

echo "== no-conflict-markers"
bash scripts/guards/no-conflict-markers.sh

echo "== sandbox-network (SEC-09: sandboxes reach gateway:4102 + runner:4105 only, previews never widen it)"
bash scripts/guards/sandbox-network.sh

echo "== no-deployment-defaults (RFE-019: neutral brief, public digest-pinned sandbox base)"
bash scripts/guards/no-deployment-defaults.sh

echo "== helm-values-docs (FIX-069: docs/23 documents every values.yaml key and names none that do not exist)"
bash scripts/guards/helm-values-docs.sh

echo "== quick-gate syntax (VEL-002; shellcheck when installed, else bash -n)"
if command -v shellcheck >/dev/null 2>&1; then
  shellcheck scripts/dev/quick-gate.sh
else
  bash -n scripts/dev/quick-gate.sh
fi

echo "== all guards passed"
